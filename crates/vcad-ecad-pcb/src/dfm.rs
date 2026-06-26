//! PCB Design-for-Manufacturing (DFM) checks against fab-house capability packs.
//!
//! Where [`crate::drc`] validates a board against its *own* declared design
//! rules, DFM validates the board's geometry against a *fab house's published
//! process capability*: a board can pass DRC (its own clearance is 0.15 mm) yet
//! be unmanufacturable on a budget process that only etches 0.2 mm.
//!
//! Each fab profile is a TOML rule pack in `lib/dfm/pcb-<profile>.toml`, bundled
//! into the binary with `include_str!` exactly like the mechanical packs in
//! `vcad-kernel-dfm`. A pack lists tunable thresholds — min annular ring, min
//! drill, min trace/space by copper weight, copper-to-edge, soldermask dam /
//! sliver, silk-over-pad, acid traps (acute copper angles), and via-in-pad. The
//! checker runs every rule present in the pack over a [`Pcb`] and returns a
//! [`PcbDfmReport`] with a per-rule pass/fail, the worst measured value, and the
//! fab profile named — the shape an agent needs to branch on a verdict.
//!
//! # Example
//!
//! ```ignore
//! use vcad_ecad_pcb::dfm::{check_dfm, PcbFabProfile};
//! let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
//! assert_eq!(report.profile, "jlcpcb");
//! if !report.passed { /* an error-severity rule failed */ }
//! ```

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use vcad_ir::ecad::{FootprintGraphic, Pad, PadShape, PadType, Pcb, PcbLayer};
use vcad_ir::Vec2;

use crate::spatial::{copper_elements, pad_geom, CopperElement, CopperGeom};

/// Distance comparison epsilon (mm). Matches the DRC engine's tolerance.
const EPS: f64 = 1e-6;
/// Cap on the number of sample locations attached to one rule result.
const MAX_LOCS: usize = 16;
/// Copper thickness of one ounce of copper, in mm.
const OZ_MM: f64 = 0.035;

// ============================================================================
// Fab profiles
// ============================================================================

/// A supported PCB fabrication profile. Each maps to a bundled rule pack.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PcbFabProfile {
    /// JLCPCB standard process (1-2 layer, 1oz).
    Jlcpcb,
    /// PCBWay standard process.
    Pcbway,
    /// Conservative generic 2-layer floor.
    Generic2Layer,
    /// Mainstream generic 4-layer floor.
    Generic4Layer,
}

impl PcbFabProfile {
    /// Parse a profile id, tolerating a `pcb_` prefix and `-`/`_` separators.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        let norm = s.trim().to_ascii_lowercase().replace('-', "_");
        let norm = norm.strip_prefix("pcb_").unwrap_or(&norm);
        match norm {
            "jlcpcb" | "jlc" => Some(Self::Jlcpcb),
            "pcbway" => Some(Self::Pcbway),
            "generic_2layer" | "generic_2" | "2layer" | "2" => Some(Self::Generic2Layer),
            "generic_4layer" | "generic_4" | "4layer" | "4" => Some(Self::Generic4Layer),
            _ => None,
        }
    }

    /// Canonical snake_case profile id.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Jlcpcb => "jlcpcb",
            Self::Pcbway => "pcbway",
            Self::Generic2Layer => "generic_2layer",
            Self::Generic4Layer => "generic_4layer",
        }
    }

    /// Raw bundled TOML source for this profile's default pack.
    pub fn pack_toml(self) -> &'static str {
        match self {
            Self::Jlcpcb => include_str!("../../../lib/dfm/pcb-jlcpcb.toml"),
            Self::Pcbway => include_str!("../../../lib/dfm/pcb-pcbway.toml"),
            Self::Generic2Layer => include_str!("../../../lib/dfm/pcb-generic-2layer.toml"),
            Self::Generic4Layer => include_str!("../../../lib/dfm/pcb-generic-4layer.toml"),
        }
    }

    /// Every supported profile, for enumeration.
    pub fn all() -> [PcbFabProfile; 4] {
        [
            Self::Jlcpcb,
            Self::Pcbway,
            Self::Generic2Layer,
            Self::Generic4Layer,
        ]
    }
}

// ============================================================================
// Rule pack (TOML)
// ============================================================================

/// A loaded PCB DFM rule pack. Mirrors the mechanical `vcad-kernel-dfm`
/// `RulePack` shape so the two stay legible side by side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PcbRulePack {
    /// Schema version string.
    #[serde(default = "default_schema")]
    pub schema: String,
    /// Pack version.
    #[serde(default = "default_version")]
    pub version: String,
    /// Process tag (always `"pcb"`).
    #[serde(default)]
    pub process: String,
    /// Fab profile id (`"jlcpcb"`, …).
    #[serde(default)]
    pub profile: String,
    /// Human-readable name shown in reports.
    pub name: String,
    /// Free-form notes.
    #[serde(default)]
    pub notes: String,
    /// Rule table keyed by rule id.
    #[serde(default)]
    pub rules: HashMap<String, Rule>,
}

fn default_schema() -> String {
    "vcad.dfm/1".to_string()
}

fn default_version() -> String {
    "1".to_string()
}

/// One rule entry. Severity plus a bag of numeric thresholds the checks read.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    /// Severity to use when the rule fails.
    #[serde(default = "default_severity")]
    pub severity: String,
    /// Free-form numeric thresholds (`min_mm`, `oz1_mm`, `min_angle_deg`, …).
    #[serde(flatten)]
    pub params: HashMap<String, toml::Value>,
}

fn default_severity() -> String {
    "error".into()
}

impl Rule {
    /// Optional numeric parameter lookup.
    fn num_opt(&self, key: &str) -> Option<f64> {
        self.params
            .get(key)
            .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|i| i as f64)))
    }

    /// Numeric parameter with a fallback.
    fn num(&self, key: &str, fallback: f64) -> f64 {
        self.num_opt(key).unwrap_or(fallback)
    }

    /// Parsed severity.
    fn severity_enum(&self) -> DfmSeverity {
        match self.severity.as_str() {
            "warning" | "warn" => DfmSeverity::Warning,
            "info" => DfmSeverity::Info,
            _ => DfmSeverity::Error,
        }
    }

    /// Trace/space threshold for a copper weight (in oz). Picks `oz{n}_mm`,
    /// degrading toward the heaviest available weight (which is the most
    /// conservative), then `min_mm`.
    fn oz_threshold(&self, oz: f64) -> Option<f64> {
        let k = oz.round().clamp(1.0, 3.0) as i64;
        self.num_opt(&format!("oz{k}_mm"))
            .or_else(|| self.num_opt("oz3_mm"))
            .or_else(|| self.num_opt("oz2_mm"))
            .or_else(|| self.num_opt("oz1_mm"))
            .or_else(|| self.num_opt("min_mm"))
    }
}

impl PcbRulePack {
    /// Parse a pack from TOML.
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// The bundled default pack for a profile.
    pub fn for_profile(profile: PcbFabProfile) -> Self {
        Self::from_toml(profile.pack_toml()).expect("bundled PCB rule pack must parse")
    }
}

// ============================================================================
// Report
// ============================================================================

/// Severity of a DFM rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DfmSeverity {
    /// Hard stop — the board cannot be built on this process.
    Error,
    /// Should be reviewed; may be acceptable or need a paid option.
    Warning,
    /// Informational.
    Info,
}

/// A representative location of a DFM finding (board coordinates, mm).
#[derive(Debug, Clone, Serialize)]
pub struct DfmLocation {
    /// X coordinate (mm).
    pub x: f64,
    /// Y coordinate (mm).
    pub y: f64,
    /// Short human label (net, ref, measured value).
    pub label: String,
}

/// The pass/fail verdict for one rule.
#[derive(Debug, Clone, Serialize)]
pub struct PcbDfmRuleResult {
    /// Rule id (`"min_annular_ring"`, …).
    pub rule: String,
    /// True when the rule passed (or had nothing to check).
    pub passed: bool,
    /// False when the board has no feature this rule applies to.
    pub applicable: bool,
    /// Severity used when the rule fails.
    pub severity: DfmSeverity,
    /// Units of `limit` / `measured` (`"mm"`, `"deg"`, `"count"`, `"layers"`).
    pub units: String,
    /// The fab-capability threshold this rule enforces.
    pub limit: f64,
    /// Worst measured value, or `None` when not applicable.
    pub measured: Option<f64>,
    /// Number of offending features.
    pub violations: usize,
    /// One-line summary.
    pub message: String,
    /// Capped sample of offending locations.
    pub locations: Vec<DfmLocation>,
}

/// The full DFM verdict for a board against one fab profile.
#[derive(Debug, Clone, Serialize)]
pub struct PcbDfmReport {
    /// Profile id (`"jlcpcb"`).
    pub profile: String,
    /// Human-readable profile name.
    pub profile_name: String,
    /// Pack version.
    pub pack_version: String,
    /// Detected outer-copper weight, in oz (used for trace/space rules).
    pub copper_weight_oz: f64,
    /// Detected copper layer count.
    pub copper_layer_count: usize,
    /// True when no error-severity rule failed.
    pub passed: bool,
    /// Number of failed error-severity rules.
    pub error_count: usize,
    /// Number of failed warning-severity rules.
    pub warning_count: usize,
    /// Per-rule results, in a stable order.
    pub rules: Vec<PcbDfmRuleResult>,
}

// ============================================================================
// Entry point
// ============================================================================

/// Run PCB DFM against a fab profile.
///
/// `override_toml`, when supplied, replaces the bundled pack (same schema) so a
/// caller can tweak thresholds per quote without editing the repo.
pub fn check_dfm(
    pcb: &Pcb,
    profile: PcbFabProfile,
    override_toml: Option<&str>,
) -> Result<PcbDfmReport, toml::de::Error> {
    let pack = match override_toml {
        Some(src) => PcbRulePack::from_toml(src)?,
        None => PcbRulePack::for_profile(profile),
    };
    Ok(run_pack(pcb, profile, &pack))
}

/// Run an already-parsed pack against a board.
pub fn run_pack(pcb: &Pcb, profile: PcbFabProfile, pack: &PcbRulePack) -> PcbDfmReport {
    let oz = detect_copper_oz(pcb);
    let layers = detect_layer_count(pcb);

    // Stable rule order: the dispatcher only emits a result for rules the pack
    // actually enables, so a fab can drop a check by deleting its table.
    let mut results = Vec::new();
    let run = |id: &str, f: &dyn Fn(&Rule) -> PcbDfmRuleResult, out: &mut Vec<PcbDfmRuleResult>| {
        if let Some(rule) = pack.rules.get(id) {
            out.push(f(rule));
        }
    };

    run(
        "min_trace_width",
        &|r| check_min_trace_width(pcb, r, oz),
        &mut results,
    );
    run(
        "min_clearance",
        &|r| check_min_clearance(pcb, r, oz),
        &mut results,
    );
    run("min_drill", &|r| check_min_drill(pcb, r), &mut results);
    run(
        "min_annular_ring",
        &|r| check_annular_ring(pcb, r),
        &mut results,
    );
    run(
        "copper_to_edge",
        &|r| check_copper_to_edge(pcb, r),
        &mut results,
    );
    run(
        "hole_to_hole",
        &|r| check_hole_to_hole(pcb, r),
        &mut results,
    );
    run(
        "soldermask_dam",
        &|r| check_soldermask(pcb, r, "soldermask_dam", "min_mm"),
        &mut results,
    );
    run(
        "soldermask_sliver",
        &|r| check_soldermask(pcb, r, "soldermask_sliver", "min_mm"),
        &mut results,
    );
    run(
        "silk_over_pad",
        &|r| check_silk_over_pad(pcb, r),
        &mut results,
    );
    run("acid_trap", &|r| check_acid_trap(pcb, r), &mut results);
    run("via_in_pad", &|r| check_via_in_pad(pcb, r), &mut results);
    run(
        "max_copper_layers",
        &|r| check_max_layers(r, layers),
        &mut results,
    );

    let error_count = results
        .iter()
        .filter(|r| !r.passed && r.severity == DfmSeverity::Error)
        .count();
    let warning_count = results
        .iter()
        .filter(|r| !r.passed && r.severity == DfmSeverity::Warning)
        .count();

    PcbDfmReport {
        profile: profile.as_str().to_string(),
        profile_name: pack.name.clone(),
        pack_version: pack.version.clone(),
        copper_weight_oz: oz,
        copper_layer_count: layers,
        passed: error_count == 0,
        error_count,
        warning_count,
        rules: results,
    }
}

// ============================================================================
// Board introspection
// ============================================================================

/// All copper layers, ordered top → bottom (mirrors the DRC stack).
const COPPER_STACK: [PcbLayer; 8] = [
    PcbLayer::FCu,
    PcbLayer::In1Cu,
    PcbLayer::In2Cu,
    PcbLayer::In3Cu,
    PcbLayer::In4Cu,
    PcbLayer::In5Cu,
    PcbLayer::In6Cu,
    PcbLayer::BCu,
];

/// Single-copper-layer bit (0 for non-copper).
fn copper_bit(layer: PcbLayer) -> u16 {
    COPPER_STACK
        .iter()
        .position(|&c| c == layer)
        .map(|i| 1u16 << i)
        .unwrap_or(0)
}

/// Detect outer copper weight in oz from the stackup (max copper thickness),
/// defaulting to 1oz when the stackup is silent.
fn detect_copper_oz(pcb: &Pcb) -> f64 {
    let max_t = pcb
        .stackup
        .layers
        .iter()
        .filter(|l| l.layer.is_copper())
        .filter_map(|l| l.copper_thickness)
        .fold(0.0_f64, f64::max);
    if max_t <= 0.0 {
        1.0
    } else {
        (max_t / OZ_MM).round().max(1.0)
    }
}

/// Detect copper layer count: from the stackup if present, else from the copper
/// layers actually carrying copper, floored at 2 (a board is at least 2-layer).
fn detect_layer_count(pcb: &Pcb) -> usize {
    let from_stackup = pcb
        .stackup
        .layers
        .iter()
        .filter(|l| l.layer.is_copper())
        .count();
    if from_stackup > 0 {
        return from_stackup;
    }
    let mut mask = 0u16;
    for t in &pcb.traces {
        mask |= copper_bit(t.layer);
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            for &l in &pad.layers {
                mask |= copper_bit(l);
            }
        }
    }
    (mask.count_ones() as usize).max(2)
}

// ============================================================================
// Geometry helpers
// ============================================================================

fn midpoint(a: Vec2, b: Vec2) -> Vec2 {
    Vec2::new((a.x + b.x) / 2.0, (a.y + b.y) / 2.0)
}

fn push_loc(locs: &mut Vec<DfmLocation>, p: Vec2, label: String) {
    if locs.len() < MAX_LOCS {
        locs.push(DfmLocation {
            x: p.x,
            y: p.y,
            label,
        });
    }
}

/// Minimum distance from a point to a closed polygon's edges.
fn min_distance_to_polygon(point: Vec2, polygon: &[Vec2]) -> f64 {
    let n = polygon.len();
    if n == 0 {
        return f64::MAX;
    }
    let mut min_d = f64::MAX;
    for i in 0..n {
        let d = point_to_segment_distance(point, polygon[i], polygon[(i + 1) % n]);
        if d < min_d {
            min_d = d;
        }
    }
    min_d
}

/// Distance from a point to a line segment.
fn point_to_segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    let len_sq = dx * dx + dy * dy;
    if len_sq < 1e-12 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len_sq).clamp(0.0, 1.0);
    let proj = Vec2::new(a.x + t * dx, a.y + t * dy);
    ((p.x - proj.x).powi(2) + (p.y - proj.y).powi(2)).sqrt()
}

/// Pad copper "radius" — half the largest dimension (for edge clearance).
fn pad_outer_radius(shape: &PadShape) -> f64 {
    match shape {
        PadShape::Circle { diameter } => diameter / 2.0,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.max(*height) / 2.0,
        PadShape::Custom { vertices } => vertices
            .iter()
            .map(|v| (v.x * v.x + v.y * v.y).sqrt())
            .fold(0.0_f64, f64::max),
    }
}

/// Minimum cross-section of a pad (for annular ring on THT pads).
fn pad_min_dimension(pad: &Pad) -> f64 {
    match &pad.shape {
        PadShape::Circle { diameter } => *diameter,
        PadShape::Rect { width, height }
        | PadShape::Oval { width, height }
        | PadShape::RoundRect { width, height, .. } => width.min(*height),
        PadShape::Custom { vertices } => {
            if vertices.is_empty() {
                return 0.0;
            }
            let (mut min_x, mut max_x, mut min_y, mut max_y) =
                (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for v in vertices {
                min_x = min_x.min(v.x);
                max_x = max_x.max(v.x);
                min_y = min_y.min(v.y);
                max_y = max_y.max(v.y);
            }
            (max_x - min_x).min(max_y - min_y)
        }
    }
}

/// Absolute board-frame center of a pad on its footprint.
fn pad_center(fp: &vcad_ir::ecad::Footprint, pad: &Pad) -> Vec2 {
    Vec2::new(
        fp.position.x + pad.position.x,
        fp.position.y + pad.position.y,
    )
}

/// Assemble a "minimum metric" rule result (smaller is worse).
#[allow(clippy::too_many_arguments)]
fn min_metric(
    rule_id: &str,
    severity: DfmSeverity,
    units: &str,
    limit: f64,
    worst: f64,
    applicable: bool,
    violations: usize,
    locations: Vec<DfmLocation>,
    message: String,
) -> PcbDfmRuleResult {
    PcbDfmRuleResult {
        rule: rule_id.to_string(),
        passed: violations == 0,
        applicable,
        severity,
        units: units.to_string(),
        limit,
        measured: if applicable { Some(worst) } else { None },
        violations,
        message,
        locations,
    }
}

// ============================================================================
// Individual checks
// ============================================================================

fn check_min_trace_width(pcb: &Pcb, rule: &Rule, oz: f64) -> PcbDfmRuleResult {
    let limit = rule.oz_threshold(oz).unwrap_or(0.0);
    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for t in &pcb.traces {
        applicable = true;
        worst = worst.min(t.width);
        if t.width < limit - EPS {
            violations += 1;
            push_loc(
                &mut locs,
                midpoint(t.start, t.end),
                format!("net '{}' trace {:.3}mm", t.net, t.width),
            );
        }
    }
    for a in &pcb.trace_arcs {
        applicable = true;
        worst = worst.min(a.width);
        if a.width < limit - EPS {
            violations += 1;
            push_loc(
                &mut locs,
                a.center,
                format!("net '{}' arc {:.3}mm", a.net, a.width),
            );
        }
    }

    let msg = if !applicable {
        "no routed copper to check".to_string()
    } else if violations == 0 {
        format!("narrowest trace {worst:.3}mm ≥ {limit:.3}mm ({oz:.0}oz)")
    } else {
        format!("{violations} trace(s) below {limit:.3}mm min ({oz:.0}oz); narrowest {worst:.3}mm")
    };
    min_metric(
        "min_trace_width",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

fn check_min_clearance(pcb: &Pcb, rule: &Rule, oz: f64) -> PcbDfmRuleResult {
    let limit = rule.oz_threshold(oz).unwrap_or(0.0);
    let elems = copper_elements(pcb);
    // Bucket by layer so we only compare coplanar copper, then pairwise within
    // each layer (i<j, different net). Mirrors the DRC engine's O(n²) pad pass;
    // fine for typical board element counts, and DFM is an on-demand check.
    let mut buckets: HashMap<PcbLayer, Vec<&CopperElement>> = HashMap::new();
    for e in &elems {
        buckets.entry(e.layer).or_default().push(e);
    }

    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for bucket in buckets.values() {
        for i in 0..bucket.len() {
            for j in (i + 1)..bucket.len() {
                let (a, b) = (bucket[i], bucket[j]);
                if a.net == b.net {
                    continue; // same net never violates spacing
                }
                applicable = true;
                let d = a.geom.distance_to(&b.geom);
                worst = worst.min(d);
                if d < limit - EPS {
                    violations += 1;
                    let pa = Vec2::new((a.min[0] + a.max[0]) / 2.0, (a.min[1] + a.max[1]) / 2.0);
                    let pb = Vec2::new((b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0);
                    push_loc(
                        &mut locs,
                        midpoint(pa, pb),
                        format!("'{}'↔'{}' {:.3}mm", a.net, b.net, d),
                    );
                }
            }
        }
    }

    let msg = if !applicable {
        "no different-net copper pairs to check".to_string()
    } else if violations == 0 {
        format!("tightest spacing {worst:.3}mm ≥ {limit:.3}mm ({oz:.0}oz)")
    } else {
        format!("{violations} copper pair(s) below {limit:.3}mm space ({oz:.0}oz); tightest {worst:.3}mm")
    };
    min_metric(
        "min_clearance",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

fn check_min_drill(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let limit = rule.num("min_mm", 0.0);
    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for via in &pcb.vias {
        applicable = true;
        worst = worst.min(via.drill);
        if via.drill < limit - EPS {
            violations += 1;
            push_loc(
                &mut locs,
                via.position,
                format!("via drill {:.3}mm", via.drill),
            );
        }
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(drill) = &pad.drill {
                applicable = true;
                worst = worst.min(drill.diameter);
                if drill.diameter < limit - EPS {
                    violations += 1;
                    push_loc(
                        &mut locs,
                        pad_center(fp, pad),
                        format!(
                            "{} pad {} drill {:.3}mm",
                            fp.reference, pad.number, drill.diameter
                        ),
                    );
                }
            }
        }
    }

    let msg = if !applicable {
        "no drilled holes to check".to_string()
    } else if violations == 0 {
        format!("smallest drill {worst:.3}mm ≥ {limit:.3}mm")
    } else {
        format!("{violations} hole(s) below {limit:.3}mm drill; smallest {worst:.3}mm")
    };
    min_metric(
        "min_drill",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

fn check_annular_ring(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let limit = rule.num("min_mm", 0.0);
    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for via in &pcb.vias {
        applicable = true;
        let ring = (via.diameter - via.drill) / 2.0;
        worst = worst.min(ring);
        if ring < limit - EPS {
            violations += 1;
            push_loc(&mut locs, via.position, format!("via ring {ring:.3}mm"));
        }
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.pad_type != PadType::THT {
                continue;
            }
            if let Some(drill) = &pad.drill {
                applicable = true;
                let ring = (pad_min_dimension(pad) - drill.diameter) / 2.0;
                worst = worst.min(ring);
                if ring < limit - EPS {
                    violations += 1;
                    push_loc(
                        &mut locs,
                        pad_center(fp, pad),
                        format!("{} pad {} ring {ring:.3}mm", fp.reference, pad.number),
                    );
                }
            }
        }
    }

    let msg = if !applicable {
        "no annular features to check".to_string()
    } else if violations == 0 {
        format!("narrowest ring {worst:.3}mm ≥ {limit:.3}mm")
    } else {
        format!("{violations} ring(s) below {limit:.3}mm; narrowest {worst:.3}mm")
    };
    min_metric(
        "min_annular_ring",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

fn check_copper_to_edge(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let limit = rule.num("min_mm", 0.0);
    let outline = &pcb.outline.vertices;
    if outline.len() < 3 {
        return min_metric(
            "copper_to_edge",
            rule.severity_enum(),
            "mm",
            limit,
            f64::INFINITY,
            false,
            0,
            Vec::new(),
            "no board outline to check against".to_string(),
        );
    }
    // All edges to keep distance from: the board outline plus any cutouts.
    let mut edges: Vec<&[Vec2]> = vec![outline.as_slice()];
    for c in &pcb.outline.cutouts {
        if c.len() >= 3 {
            edges.push(c.as_slice());
        }
    }
    let dist_to_edges = |p: Vec2| {
        edges
            .iter()
            .map(|poly| min_distance_to_polygon(p, poly))
            .fold(f64::MAX, f64::min)
    };

    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    let observe =
        |p: Vec2, halo: f64, label: String, locs: &mut Vec<DfmLocation>| -> (f64, usize) {
            let eff = dist_to_edges(p) - halo;
            let mut v = 0;
            if eff < limit - EPS {
                v = 1;
                push_loc(locs, p, label);
            }
            (eff, v)
        };

    for t in &pcb.traces {
        applicable = true;
        for pt in [t.start, t.end] {
            let (eff, v) = observe(
                pt,
                t.width / 2.0,
                format!("trace net '{}'", t.net),
                &mut locs,
            );
            worst = worst.min(eff);
            violations += v;
        }
    }
    for via in &pcb.vias {
        applicable = true;
        let (eff, v) = observe(
            via.position,
            via.diameter / 2.0,
            format!("via net '{}'", via.net),
            &mut locs,
        );
        worst = worst.min(eff);
        violations += v;
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if !pad.layers.iter().any(|l| l.is_copper()) {
                continue;
            }
            applicable = true;
            let (eff, v) = observe(
                pad_center(fp, pad),
                pad_outer_radius(&pad.shape),
                format!("{} pad {}", fp.reference, pad.number),
                &mut locs,
            );
            worst = worst.min(eff);
            violations += v;
        }
    }

    let msg = if !applicable {
        "no copper to check against the edge".to_string()
    } else if violations == 0 {
        format!("nearest copper {worst:.3}mm ≥ {limit:.3}mm from edge")
    } else {
        format!("{violations} copper feature(s) within {limit:.3}mm of edge; nearest {worst:.3}mm")
    };
    min_metric(
        "copper_to_edge",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

fn check_hole_to_hole(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let limit = rule.num("min_mm", 0.0);
    // (center, radius) for every drilled hole.
    let mut holes: Vec<(Vec2, f64)> = Vec::new();
    for via in &pcb.vias {
        holes.push((via.position, via.drill / 2.0));
    }
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if let Some(drill) = &pad.drill {
                holes.push((pad_center(fp, pad), drill.diameter / 2.0));
            }
        }
    }

    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let applicable = holes.len() >= 2;

    for i in 0..holes.len() {
        for j in (i + 1)..holes.len() {
            let center_dist = ((holes[i].0.x - holes[j].0.x).powi(2)
                + (holes[i].0.y - holes[j].0.y).powi(2))
            .sqrt();
            let edge_dist = center_dist - holes[i].1 - holes[j].1;
            worst = worst.min(edge_dist);
            if edge_dist < limit - EPS {
                violations += 1;
                push_loc(
                    &mut locs,
                    midpoint(holes[i].0, holes[j].0),
                    format!("hole gap {edge_dist:.3}mm"),
                );
            }
        }
    }

    let msg = if !applicable {
        "fewer than two holes to check".to_string()
    } else if violations == 0 {
        format!("closest holes {worst:.3}mm ≥ {limit:.3}mm apart")
    } else {
        format!("{violations} hole pair(s) below {limit:.3}mm; closest {worst:.3}mm")
    };
    min_metric(
        "hole_to_hole",
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

/// Solder-mask web (dam / sliver) between adjacent mask openings.
///
/// Models each exposed pad as a copper island; the mask web between two
/// inter-footprint openings on the same board side is their copper edge gap
/// minus twice the mask expansion. A web below the rule's threshold can't print
/// a reliable dam (bridging risk) or leaves a fragile sliver.
fn check_soldermask(pcb: &Pcb, rule: &Rule, rule_id: &str, limit_key: &str) -> PcbDfmRuleResult {
    let limit = rule.num(limit_key, 0.0);
    let expansion = rule.num("expansion_mm", 0.0);

    struct PadIsland {
        fp: usize,
        front: bool,
        geom: CopperGeom,
        center: Vec2,
        label: String,
    }
    let mut islands: Vec<PadIsland> = Vec::new();
    for (fi, fp) in pcb.footprints.iter().enumerate() {
        for pad in &fp.pads {
            if !pad.layers.iter().any(|l| l.is_copper()) {
                continue;
            }
            let center = pad_center(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            islands.push(PadIsland {
                fp: fi,
                front: fp.front,
                geom: pad_geom(pad, center, rot),
                center,
                label: format!("{} pad {}", fp.reference, pad.number),
            });
        }
    }

    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for i in 0..islands.len() {
        for j in (i + 1)..islands.len() {
            let (a, b) = (&islands[i], &islands[j]);
            // Mask is per side; intra-footprint land patterns are the
            // footprint's concern (matches the DRC pad-clearance exemption).
            if a.front != b.front || a.fp == b.fp {
                continue;
            }
            applicable = true;
            let web = a.geom.distance_to(&b.geom) - 2.0 * expansion;
            worst = worst.min(web);
            if web < limit - EPS {
                violations += 1;
                push_loc(
                    &mut locs,
                    midpoint(a.center, b.center),
                    format!("{} ↔ {} web {:.3}mm", a.label, b.label, web),
                );
            }
        }
    }

    let msg = if !applicable {
        "no adjacent mask openings to check".to_string()
    } else if violations == 0 {
        format!("thinnest mask web {worst:.3}mm ≥ {limit:.3}mm")
    } else {
        format!("{violations} mask web(s) below {limit:.3}mm; thinnest {worst:.3}mm")
    };
    min_metric(
        rule_id,
        rule.severity_enum(),
        "mm",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

/// Silkscreen strokes (footprint-local) lowered into world-space segments,
/// tagged with the side (`true` = front) they print on.
fn silk_segments(fp: &vcad_ir::ecad::Footprint) -> Vec<(Vec2, Vec2, f64, bool)> {
    let rot = fp.rotation.to_radians();
    let (s, c) = rot.sin_cos();
    let to_world = |p: Vec2| {
        Vec2::new(
            fp.position.x + p.x * c - p.y * s,
            fp.position.y + p.x * s + p.y * c,
        )
    };
    let side = |layer: PcbLayer| match layer {
        PcbLayer::FSilkS => Some(true),
        PcbLayer::BSilkS => Some(false),
        _ => None,
    };

    let mut out = Vec::new();
    for g in &fp.graphics {
        match g {
            FootprintGraphic::Line {
                start,
                end,
                width,
                layer,
            } => {
                if let Some(front) = side(*layer) {
                    out.push((to_world(*start), to_world(*end), *width, front));
                }
            }
            FootprintGraphic::Rect {
                start,
                end,
                width,
                layer,
            } => {
                if let Some(front) = side(*layer) {
                    let corners = [
                        Vec2::new(start.x, start.y),
                        Vec2::new(end.x, start.y),
                        Vec2::new(end.x, end.y),
                        Vec2::new(start.x, end.y),
                    ];
                    for k in 0..4 {
                        out.push((
                            to_world(corners[k]),
                            to_world(corners[(k + 1) % 4]),
                            *width,
                            front,
                        ));
                    }
                }
            }
            FootprintGraphic::Polygon {
                vertices,
                width,
                layer,
            } => {
                if let Some(front) = side(*layer) {
                    let n = vertices.len();
                    for k in 0..n {
                        out.push((
                            to_world(vertices[k]),
                            to_world(vertices[(k + 1) % n]),
                            *width,
                            front,
                        ));
                    }
                }
            }
            FootprintGraphic::Circle {
                center,
                radius,
                width,
                layer,
            } => {
                if let Some(front) = side(*layer) {
                    sample_arc(
                        *center, *radius, 0.0, 360.0, *width, front, to_world, &mut out,
                    );
                }
            }
            FootprintGraphic::Arc {
                center,
                radius,
                start_angle,
                end_angle,
                width,
                layer,
            } => {
                if let Some(front) = side(*layer) {
                    sample_arc(
                        *center,
                        *radius,
                        *start_angle,
                        *end_angle,
                        *width,
                        front,
                        to_world,
                        &mut out,
                    );
                }
            }
            // Text bounding box is its own concern; skip — silk text over copper
            // is caught by the line/poly strokes that bound the reference.
            FootprintGraphic::Text { .. } => {}
        }
    }
    out
}

/// Tessellate an arc/circle into world-space silk segments.
#[allow(clippy::too_many_arguments)]
fn sample_arc(
    center: Vec2,
    radius: f64,
    start_deg: f64,
    end_deg: f64,
    width: f64,
    front: bool,
    to_world: impl Fn(Vec2) -> Vec2,
    out: &mut Vec<(Vec2, Vec2, f64, bool)>,
) {
    let steps = 16usize;
    let (a0, a1) = (start_deg.to_radians(), end_deg.to_radians());
    let mut prev = None;
    for k in 0..=steps {
        let t = a0 + (a1 - a0) * (k as f64 / steps as f64);
        let p = to_world(Vec2::new(
            center.x + radius * t.cos(),
            center.y + radius * t.sin(),
        ));
        if let Some(prev) = prev {
            out.push((prev, p, width, front));
        }
        prev = Some(p);
    }
}

fn check_silk_over_pad(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let clearance = rule.num("clearance_mm", 0.0);

    // Exposed pad copper per side.
    struct PadIsland {
        front: bool,
        geom: CopperGeom,
        center: Vec2,
        label: String,
    }
    let mut pads: Vec<PadIsland> = Vec::new();
    for fp in &pcb.footprints {
        for pad in &fp.pads {
            let front = pad.layers.contains(&PcbLayer::FCu);
            let back = pad.layers.contains(&PcbLayer::BCu);
            if !front && !back {
                continue;
            }
            let center = pad_center(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            let geom = pad_geom(pad, center, rot);
            if front {
                pads.push(PadIsland {
                    front: true,
                    geom,
                    center,
                    label: format!("{} pad {}", fp.reference, pad.number),
                });
            }
            if back {
                pads.push(PadIsland {
                    front: false,
                    geom,
                    center,
                    label: format!("{} pad {}", fp.reference, pad.number),
                });
            }
        }
    }

    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for fp in &pcb.footprints {
        for (sa, sb, w, front) in silk_segments(fp) {
            let seg = CopperGeom::Segment {
                a: sa,
                b: sb,
                half_w: w / 2.0,
            };
            for pad in &pads {
                if pad.front != front {
                    continue;
                }
                applicable = true;
                let d = seg.distance_to(&pad.geom);
                worst = worst.min(d);
                if d < clearance - EPS {
                    violations += 1;
                    push_loc(
                        &mut locs,
                        pad.center,
                        format!("silk over {} ({d:.3}mm)", pad.label),
                    );
                }
            }
        }
    }

    let msg = if !applicable {
        "no silkscreen over pads to check".to_string()
    } else if violations == 0 {
        format!("silk-to-pad clearance {worst:.3}mm ≥ {clearance:.3}mm")
    } else {
        format!("{violations} silk stroke(s) within {clearance:.3}mm of a pad; worst {worst:.3}mm")
    };
    min_metric(
        "silk_over_pad",
        rule.severity_enum(),
        "mm",
        clearance,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

/// Interior copper angle (degrees) at trace junctions and zone-outline corners.
/// An angle below the threshold etches as an acid trap.
fn check_acid_trap(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    let limit = rule.num("min_angle_deg", 90.0);
    let mut worst = f64::INFINITY;
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    // 1. Trace junctions: outgoing directions sharing a (vertex, net, layer).
    let q = |v: f64| (v * 1000.0).round() as i64;
    let mut junctions: HashMap<(i64, i64, u16, String), Vec<Vec2>> = HashMap::new();
    let mut junction_pt: HashMap<(i64, i64, u16, String), Vec2> = HashMap::new();
    for t in &pcb.traces {
        let bit = copper_bit(t.layer);
        for (v, other) in [(t.start, t.end), (t.end, t.start)] {
            let key = (q(v.x), q(v.y), bit, t.net.clone());
            let dir = Vec2::new(other.x - v.x, other.y - v.y);
            if dir.x.abs() < EPS && dir.y.abs() < EPS {
                continue;
            }
            junctions.entry(key.clone()).or_default().push(dir);
            junction_pt.entry(key).or_insert(v);
        }
    }
    for (key, dirs) in &junctions {
        if dirs.len() < 2 {
            continue;
        }
        applicable = true;
        let v = junction_pt[key];
        for i in 0..dirs.len() {
            for j in (i + 1)..dirs.len() {
                let ang = angle_between(dirs[i], dirs[j]);
                worst = worst.min(ang);
                if ang < limit - EPS {
                    violations += 1;
                    push_loc(&mut locs, v, format!("trace junction {ang:.1}°"));
                }
            }
        }
    }

    // 2. Zone outline corners: interior angle at each vertex.
    for zone in &pcb.zones {
        let poly = &zone.outline;
        let n = poly.len();
        if n < 3 {
            continue;
        }
        applicable = true;
        for k in 0..n {
            let prev = poly[(k + n - 1) % n];
            let cur = poly[k];
            let next = poly[(k + 1) % n];
            let d1 = Vec2::new(prev.x - cur.x, prev.y - cur.y);
            let d2 = Vec2::new(next.x - cur.x, next.y - cur.y);
            let ang = angle_between(d1, d2);
            worst = worst.min(ang);
            if ang < limit - EPS {
                violations += 1;
                push_loc(&mut locs, cur, format!("zone corner {ang:.1}°"));
            }
        }
    }

    let msg = if !applicable {
        "no trace junctions or zone corners to check".to_string()
    } else if violations == 0 {
        format!("sharpest copper angle {worst:.1}° ≥ {limit:.1}°")
    } else {
        format!("{violations} acute copper angle(s) below {limit:.1}°; sharpest {worst:.1}°")
    };
    min_metric(
        "acid_trap",
        rule.severity_enum(),
        "deg",
        limit,
        worst,
        applicable,
        violations,
        locs,
        msg,
    )
}

/// Angle between two vectors in degrees (0..180).
fn angle_between(a: Vec2, b: Vec2) -> f64 {
    let la = (a.x * a.x + a.y * a.y).sqrt();
    let lb = (b.x * b.x + b.y * b.y).sqrt();
    if la < EPS || lb < EPS {
        return 180.0;
    }
    let cos = ((a.x * b.x + a.y * b.y) / (la * lb)).clamp(-1.0, 1.0);
    cos.acos().to_degrees()
}

fn check_via_in_pad(pcb: &Pcb, rule: &Rule) -> PcbDfmRuleResult {
    // Count vias whose copper sits inside an SMD pad on a shared layer.
    let mut violations = 0;
    let mut locs = Vec::new();
    let mut applicable = false;

    for fp in &pcb.footprints {
        for pad in &fp.pads {
            if pad.pad_type != PadType::SMD {
                continue;
            }
            let center = pad_center(fp, pad);
            let rot = (fp.rotation + pad.rotation).to_radians();
            let geom = pad_geom(pad, center, rot);
            for via in &pcb.vias {
                applicable = true;
                let shares_layer = pad
                    .layers
                    .iter()
                    .any(|l| *l == via.start_layer || *l == via.end_layer);
                if !shares_layer {
                    continue;
                }
                let via_geom = CopperGeom::Disc {
                    center: via.position,
                    r: via.diameter / 2.0,
                };
                if geom.distance_to(&via_geom) <= EPS {
                    violations += 1;
                    push_loc(
                        &mut locs,
                        via.position,
                        format!("via in {} pad {}", fp.reference, pad.number),
                    );
                }
            }
        }
    }

    let measured = violations as f64;
    let severity = rule.severity_enum();
    let passed = violations == 0;
    let msg = if !applicable {
        "no SMD pads or vias to check".to_string()
    } else if passed {
        "no via-in-pad".to_string()
    } else {
        format!("{violations} via(s) land inside an SMD pad (requires filled/capped via service)")
    };
    PcbDfmRuleResult {
        rule: "via_in_pad".to_string(),
        passed,
        applicable,
        severity,
        units: "count".to_string(),
        limit: 0.0,
        measured: if applicable { Some(measured) } else { None },
        violations,
        message: msg,
        locations: locs,
    }
}

fn check_max_layers(rule: &Rule, layers: usize) -> PcbDfmRuleResult {
    let limit = rule.num("max", 8.0);
    let passed = (layers as f64) <= limit + EPS;
    let msg = if passed {
        format!("{layers}-layer board within {limit:.0}-layer max")
    } else {
        format!("{layers}-layer board exceeds the {limit:.0}-layer max for this profile")
    };
    PcbDfmRuleResult {
        rule: "max_copper_layers".to_string(),
        passed,
        applicable: true,
        severity: rule.severity_enum(),
        units: "layers".to_string(),
        limit,
        measured: Some(layers as f64),
        violations: if passed { 0 } else { 1 },
        message: msg,
        locations: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::ecad::*;
    use vcad_ir::Vec2;

    fn rect(w: f64, h: f64) -> BoardOutline {
        BoardOutline {
            vertices: vec![
                Vec2::new(0.0, 0.0),
                Vec2::new(w, 0.0),
                Vec2::new(w, h),
                Vec2::new(0.0, h),
            ],
            cutouts: vec![],
            thickness: 1.6,
        }
    }

    fn default_rules() -> DesignRules {
        DesignRules {
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
            net_class_assignments: std::collections::HashMap::new(),
            edge_clearance: 0.2,
            hole_to_hole: 0.5,
            min_annular_ring: 0.13,
            min_drill: 0.2,
        }
    }

    fn two_layer_stackup() -> LayerStackup {
        LayerStackup {
            layers: vec![
                StackupLayer {
                    layer: PcbLayer::FCu,
                    copper_thickness: Some(0.035),
                    dielectric_thickness: Some(1.5),
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
        }
    }

    fn base_pcb() -> Pcb {
        Pcb {
            outline: rect(50.0, 40.0),
            stackup: two_layer_stackup(),
            nets: vec![],
            rules: default_rules(),
            footprints: vec![],
            traces: vec![],
            trace_arcs: vec![],
            vias: vec![],
            zones: vec![],
            keepouts: vec![],
            net_ties: vec![],
        }
    }

    fn find<'a>(report: &'a PcbDfmReport, rule: &str) -> &'a PcbDfmRuleResult {
        report
            .rules
            .iter()
            .find(|r| r.rule == rule)
            .unwrap_or_else(|| panic!("rule {rule} missing from report"))
    }

    #[test]
    fn all_bundled_packs_parse() {
        for p in PcbFabProfile::all() {
            let pack = PcbRulePack::for_profile(p);
            assert_eq!(pack.profile, p.as_str(), "profile id mismatch for {p:?}");
            assert!(!pack.rules.is_empty(), "{p:?} pack has no rules");
        }
    }

    #[test]
    fn profile_parsing_tolerates_prefixes() {
        assert_eq!(
            PcbFabProfile::from_str("pcb_jlcpcb"),
            Some(PcbFabProfile::Jlcpcb)
        );
        assert_eq!(PcbFabProfile::from_str("JLC"), Some(PcbFabProfile::Jlcpcb));
        assert_eq!(
            PcbFabProfile::from_str("generic-2layer"),
            Some(PcbFabProfile::Generic2Layer)
        );
        assert_eq!(PcbFabProfile::from_str("nope"), None);
    }

    #[test]
    fn clean_board_passes_jlcpcb() {
        // A trivially clean board: one wide trace well inside the edges.
        let mut pcb = base_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(10.0, 10.0),
            end: Vec2::new(30.0, 10.0),
            width: 0.3,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
        assert_eq!(report.profile, "jlcpcb");
        assert_eq!(report.copper_weight_oz, 1.0);
        assert!(
            report.passed,
            "clean board should pass: {:#?}",
            report.rules
        );
        assert!(find(&report, "min_trace_width").passed);
    }

    #[test]
    fn violating_board_fails_named_rules() {
        let mut pcb = base_pcb();
        // 1) A hair-thin trace (0.08mm) — below JLC 0.127mm min trace width.
        pcb.traces.push(Trace {
            start: Vec2::new(10.0, 10.0),
            end: Vec2::new(20.0, 10.0),
            width: 0.08,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        // 2) A via with a 0.1mm drill — below JLC 0.2mm min drill, and a
        //    0.05mm annular ring (0.2 dia / 0.1 drill) below 0.13mm.
        pcb.vias.push(Via {
            position: Vec2::new(25.0, 20.0),
            diameter: 0.2,
            drill: 0.1,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "SIG".into(),
        });

        let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
        assert!(!report.passed, "board with violations should fail");

        let tw = find(&report, "min_trace_width");
        assert!(!tw.passed);
        assert_eq!(tw.severity, DfmSeverity::Error);
        assert!((tw.limit - 0.127).abs() < 1e-9);
        assert_eq!(tw.measured, Some(0.08));
        assert_eq!(tw.violations, 1);

        let drill = find(&report, "min_drill");
        assert!(!drill.passed);
        assert_eq!(drill.measured, Some(0.1));

        let ring = find(&report, "min_annular_ring");
        assert!(!ring.passed, "0.05mm ring is below 0.13mm");

        // The named profile is carried on the report.
        assert_eq!(report.profile_name, "JLCPCB standard (1-2 layer, 1oz)");
        assert!(report.error_count >= 2);
    }

    #[test]
    fn copper_weight_drives_trace_minimum() {
        // 2oz outer copper raises JLC's min trace from 0.127 to 0.20mm, so a
        // 0.15mm trace that passes at 1oz now fails.
        let mut pcb = base_pcb();
        pcb.stackup.layers[0].copper_thickness = Some(0.070); // 2oz
        pcb.stackup.layers[1].copper_thickness = Some(0.070);
        pcb.traces.push(Trace {
            start: Vec2::new(10.0, 10.0),
            end: Vec2::new(30.0, 10.0),
            width: 0.15,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
        assert_eq!(report.copper_weight_oz, 2.0);
        let tw = find(&report, "min_trace_width");
        assert!((tw.limit - 0.20).abs() < 1e-9, "2oz min should be 0.20mm");
        assert!(!tw.passed, "0.15mm trace fails at 2oz");
    }

    #[test]
    fn via_in_pad_flagged_and_severity_varies_by_profile() {
        let mut pcb = base_pcb();
        // An SMD pad with a via landing on it.
        pcb.footprints.push(Footprint {
            reference: "U1".into(),
            value: "".into(),
            footprint_name: "QFN".into(),
            position: Vec2::new(20.0, 20.0),
            rotation: 0.0,
            front: true,
            pads: vec![Pad {
                number: "1".into(),
                pad_type: PadType::SMD,
                shape: PadShape::Rect {
                    width: 1.0,
                    height: 1.0,
                },
                position: Vec2::new(0.0, 0.0),
                rotation: 0.0,
                drill: None,
                net: Some("SIG".into()),
                layers: vec![PcbLayer::FCu],
            }],
            graphics: vec![],
            model_3d: None,
            properties: std::collections::HashMap::new(),
        });
        pcb.vias.push(Via {
            position: Vec2::new(20.0, 20.0),
            diameter: 0.4,
            drill: 0.2,
            start_layer: PcbLayer::FCu,
            end_layer: PcbLayer::BCu,
            net: "SIG".into(),
        });

        let jlc = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
        let vip = find(&jlc, "via_in_pad");
        assert!(!vip.passed);
        assert_eq!(
            vip.severity,
            DfmSeverity::Warning,
            "JLC: warning (POFV add-on)"
        );
        // JLC keeps `passed` true overall because via-in-pad is only a warning.
        // The generic 2-layer pack escalates it to an error.
        let gen2 = check_dfm(&pcb, PcbFabProfile::Generic2Layer, None).unwrap();
        let vip2 = find(&gen2, "via_in_pad");
        assert_eq!(vip2.severity, DfmSeverity::Error);
        assert!(
            !gen2.passed,
            "generic-2layer treats via-in-pad as a hard error"
        );
    }

    #[test]
    fn four_layer_board_exceeds_generic_2layer_max() {
        let mut pcb = base_pcb();
        pcb.stackup.layers = vec![
            StackupLayer {
                layer: PcbLayer::FCu,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(0.2),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            },
            StackupLayer {
                layer: PcbLayer::In1Cu,
                copper_thickness: Some(0.035),
                dielectric_thickness: Some(0.2),
                dielectric_er: Some(4.5),
                material: Some("FR4".into()),
            },
            StackupLayer {
                layer: PcbLayer::In2Cu,
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
        ];
        let report = check_dfm(&pcb, PcbFabProfile::Generic2Layer, None).unwrap();
        assert_eq!(report.copper_layer_count, 4);
        let ml = find(&report, "max_copper_layers");
        assert!(!ml.passed);
        assert!(!report.passed);
        // The same board is fine on the 4-layer profile.
        let ok = check_dfm(&pcb, PcbFabProfile::Generic4Layer, None).unwrap();
        assert!(find(&ok, "max_copper_layers").passed);
    }

    #[test]
    fn acid_trap_catches_hairpin_junction() {
        let mut pcb = base_pcb();
        // Two traces meeting at (20,20) with a ~37° angle between them.
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 20.0),
            end: Vec2::new(30.0, 20.0),
            width: 0.3,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        pcb.traces.push(Trace {
            start: Vec2::new(20.0, 20.0),
            end: Vec2::new(30.0, 27.5),
            width: 0.3,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, None).unwrap();
        let at = find(&report, "acid_trap");
        assert!(at.applicable);
        assert!(!at.passed, "a sub-90° junction is an acid trap");
        assert!(at.measured.unwrap() < 90.0);
    }

    #[test]
    fn override_toml_relaxes_threshold() {
        let mut pcb = base_pcb();
        pcb.traces.push(Trace {
            start: Vec2::new(10.0, 10.0),
            end: Vec2::new(20.0, 10.0),
            width: 0.08,
            layer: PcbLayer::FCu,
            net: "SIG".into(),
        });
        // A bespoke pack that allows 0.05mm traces.
        let custom = r#"
            process = "pcb"
            profile = "jlcpcb"
            name = "Custom fine-line"
            [rules.min_trace_width]
            severity = "error"
            oz1_mm = 0.05
        "#;
        let report = check_dfm(&pcb, PcbFabProfile::Jlcpcb, Some(custom)).unwrap();
        assert!(find(&report, "min_trace_width").passed);
        assert_eq!(report.profile_name, "Custom fine-line");
    }
}
