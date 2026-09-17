//! Tool definitions for CAM operations.

use serde::{Deserialize, Serialize};

/// A cutting tool definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum Tool {
    /// Flat end mill for general machining.
    FlatEndMill {
        /// Tool diameter in mm.
        diameter: f64,
        /// Flute length (cutting depth) in mm.
        flute_length: f64,
        /// Number of flutes.
        flutes: u8,
    },
    /// Ball end mill for 3D contouring.
    BallEndMill {
        /// Tool diameter in mm.
        diameter: f64,
        /// Flute length in mm.
        flute_length: f64,
        /// Number of flutes.
        flutes: u8,
    },
    /// Bull end mill (corner radius) for 3D machining.
    BullEndMill {
        /// Tool diameter in mm.
        diameter: f64,
        /// Corner radius in mm.
        corner_radius: f64,
        /// Flute length in mm.
        flute_length: f64,
        /// Number of flutes.
        flutes: u8,
    },
    /// V-bit for engraving and chamfering.
    VBit {
        /// Tool diameter at widest point in mm.
        diameter: f64,
        /// Included angle in degrees (e.g., 60, 90).
        angle: f64,
    },
    /// Drill bit for hole making.
    Drill {
        /// Drill diameter in mm.
        diameter: f64,
        /// Point angle in degrees (typically 118 or 135).
        point_angle: f64,
    },
    /// Face mill for surface machining.
    FaceMill {
        /// Cutter diameter in mm.
        diameter: f64,
        /// Number of inserts.
        inserts: u8,
    },
}

/// Tool holder definition for collision detection.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolHolder {
    /// Holder diameter in mm.
    pub diameter: f64,
    /// Holder length (from spindle face to tool tip) in mm.
    pub length: f64,
    /// Taper angle in degrees (0 for cylindrical).
    #[serde(default)]
    pub taper_angle: f64,
}

/// The part of a tool assembly the `Tool` enum does not describe: how much of
/// it cuts, how much of it hangs out of the collet, and what sits above it.
///
/// Every field is optional and serde-defaulted, so a library written before
/// this struct existed still loads. Unknown is not the same as safe: the
/// checks below report what they could not verify instead of assuming it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolGeometry {
    /// Cutting (flute) length in mm. Falls back to the tool's own flute
    /// length when the tool variant carries one.
    pub flute_length: Option<f64>,
    /// Stickout from the collet/holder face to the tool tip, in mm. This is
    /// what limits how deep the assembly can reach before the holder lands on
    /// the stock.
    pub stickout: Option<f64>,
    /// Shank diameter above the flutes, in mm. Falls back to the cutting
    /// diameter (a plain, un-necked tool).
    pub shank_diameter: Option<f64>,
    /// Whether the tool cuts across its own centre, i.e. whether it can be
    /// plunged straight down. Falls back to what the tool type implies.
    pub centre_cutting: Option<bool>,
    /// The holder this tool sits in.
    pub holder: Option<ToolHolder>,
}

/// How bad a [`ToolCheck`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckSeverity {
    /// The cut is still possible but the operator should know.
    Warning,
    /// The cut must not run as described.
    Error,
}

/// What a [`ToolCheck`] found, with the numbers that produced it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ToolCheckKind {
    /// Neither the tool nor its geometry says how long the flutes are.
    FluteLengthUnknown,
    /// The cut reaches deeper than the flutes are long.
    DepthBeyondFluteLength {
        /// Depth of cut in mm.
        depth: f64,
        /// Flute length in mm.
        flute_length: f64,
    },
    /// Past the end of the flutes the wall runs against the shank.
    ShankRub {
        /// Depth of cut in mm.
        depth: f64,
        /// Flute length in mm.
        flute_length: f64,
        /// Shank diameter in mm.
        shank_diameter: f64,
        /// Cutting diameter in mm.
        cutter_diameter: f64,
    },
    /// The assembly's stickout is not declared, so holder clearance over the
    /// stock cannot be checked.
    StickoutUnknown,
    /// The holder reaches the stock before the tool reaches depth.
    HolderIntoStock {
        /// Depth of cut in mm.
        depth: f64,
        /// Declared stickout in mm.
        stickout: f64,
        /// Clearance the holder must keep above the stock top, in mm.
        clearance: f64,
    },
    /// A full-width cut deeper than the cutter can be pushed through.
    SlenderSlot {
        /// Depth of cut in mm.
        depth: f64,
        /// Cutting diameter in mm.
        diameter: f64,
        /// depth / diameter.
        ratio: f64,
        /// The ratio at which this warning starts.
        threshold: f64,
    },
}

/// One finding from the tool-geometry checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolCheck {
    /// How bad it is.
    pub severity: CheckSeverity,
    /// What was found.
    pub kind: ToolCheckKind,
    /// A sentence a machinist can act on.
    pub message: String,
}

/// The cut a tool is being asked to make, as far as the tool checks care.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CutContext {
    /// How far below the stock top the flutes reach, in mm (positive down).
    pub depth: f64,
    /// Whether the cutter is buried full width — a slot, a drilled hole, a
    /// helix entry — as opposed to taking a side step.
    pub slotting: bool,
    /// The gap the holder must keep above the stock top, in mm.
    pub holder_clearance: f64,
}

impl CutContext {
    /// A side cut `depth` mm deep, with 2 mm of holder clearance.
    pub fn new(depth: f64) -> Self {
        Self {
            depth,
            slotting: false,
            holder_clearance: 2.0,
        }
    }

    /// Mark the cut as full-width (slot, hole, helix entry).
    pub fn slotting(mut self) -> Self {
        self.slotting = true;
        self
    }

    /// Set the clearance the holder must keep above the stock top.
    pub fn with_holder_clearance(mut self, clearance: f64) -> Self {
        self.holder_clearance = clearance;
        self
    }
}

impl ToolGeometry {
    /// An all-unknown geometry: every answer falls back to the tool type.
    pub fn new() -> Self {
        Self::default()
    }

    /// Declare the cutting length.
    pub fn with_flute_length(mut self, flute_length: f64) -> Self {
        self.flute_length = Some(flute_length);
        self
    }

    /// Declare the stickout from the collet face to the tip.
    pub fn with_stickout(mut self, stickout: f64) -> Self {
        self.stickout = Some(stickout);
        self
    }

    /// Declare the shank diameter.
    pub fn with_shank_diameter(mut self, shank_diameter: f64) -> Self {
        self.shank_diameter = Some(shank_diameter);
        self
    }

    /// Declare whether the tool cuts across its own centre.
    pub fn with_centre_cutting(mut self, centre_cutting: bool) -> Self {
        self.centre_cutting = Some(centre_cutting);
        self
    }

    /// Declare the holder.
    pub fn with_holder(mut self, holder: ToolHolder) -> Self {
        self.holder = Some(holder);
        self
    }

    /// Cutting length: what was declared, else what the tool variant carries.
    pub fn flute_length_of(&self, tool: &Tool) -> Option<f64> {
        self.flute_length.or_else(|| tool.max_depth())
    }

    /// Shank diameter: what was declared, else the cutting diameter.
    pub fn shank_diameter_of(&self, tool: &Tool) -> f64 {
        self.shank_diameter.unwrap_or_else(|| tool.diameter())
    }

    /// Whether the tool can be plunged straight down.
    ///
    /// `None` means the tool type does not settle it — a flat or bull end
    /// mill is centre-cutting only if its maker says so — and a caller that
    /// needs a plunge must refuse rather than guess.
    pub fn centre_cutting_of(&self, tool: &Tool) -> Option<bool> {
        if let Some(declared) = self.centre_cutting {
            return Some(declared);
        }
        match tool {
            // A twist drill and a V-bit come to a point; a ball nose cuts
            // through its own axis.
            Tool::Drill { .. } | Tool::VBit { .. } | Tool::BallEndMill { .. } => Some(true),
            // Inserts sit off-axis: a face mill never plunges.
            Tool::FaceMill { .. } => Some(false),
            Tool::FlatEndMill { .. } | Tool::BullEndMill { .. } => None,
        }
    }
}

/// Flute-length findings: does the cut fit inside the cutting edges, and if
/// not, what rubs.
pub fn check_flute_length(
    tool: &Tool,
    geometry: &ToolGeometry,
    cut: &CutContext,
) -> Vec<ToolCheck> {
    let Some(flute_length) = geometry.flute_length_of(tool) else {
        return vec![ToolCheck {
            severity: CheckSeverity::Warning,
            kind: ToolCheckKind::FluteLengthUnknown,
            message: format!(
                "cut is {:.2} mm deep but the tool's flute length is not declared, so it cannot be checked",
                cut.depth
            ),
        }];
    };
    if cut.depth <= flute_length {
        return Vec::new();
    }
    let shank = geometry.shank_diameter_of(tool);
    let cutter = tool.diameter();
    let mut checks = vec![ToolCheck {
        severity: CheckSeverity::Error,
        kind: ToolCheckKind::DepthBeyondFluteLength {
            depth: cut.depth,
            flute_length,
        },
        message: format!(
            "cut is {:.2} mm deep, {:.2} mm past the {:.2} mm flutes: use a longer tool or cut shallower",
            cut.depth,
            cut.depth - flute_length,
            flute_length
        ),
    }];
    // A necked tool can pass its own cut; a plain one drags its shank along
    // the wall it just made.
    checks.push(if shank >= cutter - 1e-9 {
        ToolCheck {
            severity: CheckSeverity::Error,
            kind: ToolCheckKind::ShankRub {
                depth: cut.depth,
                flute_length,
                shank_diameter: shank,
                cutter_diameter: cutter,
            },
            message: format!(
                "below {:.2} mm the Ø{:.2} shank rubs the wall the Ø{:.2} flutes cut",
                flute_length, shank, cutter
            ),
        }
    } else {
        ToolCheck {
            severity: CheckSeverity::Warning,
            kind: ToolCheckKind::ShankRub {
                depth: cut.depth,
                flute_length,
                shank_diameter: shank,
                cutter_diameter: cutter,
            },
            message: format!(
                "below {:.2} mm only the necked Ø{:.2} shank is in the Ø{:.2} cut ({:.2} mm clearance per side)",
                flute_length,
                shank,
                cutter,
                (cutter - shank) / 2.0
            ),
        }
    });
    checks
}

/// Stickout findings: does the holder stay clear of the stock top.
pub fn check_stickout(_tool: &Tool, geometry: &ToolGeometry, cut: &CutContext) -> Vec<ToolCheck> {
    let Some(stickout) = geometry.stickout else {
        return vec![ToolCheck {
            severity: CheckSeverity::Warning,
            kind: ToolCheckKind::StickoutUnknown,
            message: format!(
                "cut is {:.2} mm deep but the tool's stickout is not declared, so holder clearance over the stock cannot be checked",
                cut.depth
            ),
        }];
    };
    let required = cut.depth + cut.holder_clearance;
    if stickout >= required {
        return Vec::new();
    }
    vec![ToolCheck {
        severity: CheckSeverity::Error,
        kind: ToolCheckKind::HolderIntoStock {
            depth: cut.depth,
            stickout,
            clearance: cut.holder_clearance,
        },
        message: format!(
            "{:.2} mm of stickout reaches {:.2} mm deep with {:.2} mm of holder clearance, {:.2} mm short of the {:.2} mm cut: pull the tool out further",
            stickout,
            stickout - cut.holder_clearance,
            cut.holder_clearance,
            required - stickout,
            cut.depth
        ),
    }]
}

/// Slot findings: a full-width cut that is deep for its diameter.
///
/// Small cutters get the tighter threshold, because that is where a slot that
/// looks fine on paper snaps the tool.
pub fn check_slot_ratio(tool: &Tool, cut: &CutContext) -> Vec<ToolCheck> {
    if !cut.slotting {
        return Vec::new();
    }
    let diameter = tool.diameter();
    if diameter <= 0.0 {
        return Vec::new();
    }
    let threshold = if diameter <= 2.0 { 3.0 } else { 5.0 };
    let ratio = cut.depth / diameter;
    if ratio <= threshold {
        return Vec::new();
    }
    vec![ToolCheck {
        severity: CheckSeverity::Warning,
        kind: ToolCheckKind::SlenderSlot {
            depth: cut.depth,
            diameter,
            ratio,
            threshold,
        },
        message: format!(
            "slotting {:.2} mm deep with a Ø{:.2} cutter is {:.1}×D (over {:.0}×D): peck it, or step down in passes",
            cut.depth, diameter, ratio, threshold
        ),
    }]
}

/// Run every tool-geometry check for one cut.
pub fn check_tool_for_cut(
    tool: &Tool,
    geometry: &ToolGeometry,
    cut: &CutContext,
) -> Vec<ToolCheck> {
    let mut checks = check_flute_length(tool, geometry, cut);
    checks.extend(check_stickout(tool, geometry, cut));
    checks.extend(check_slot_ratio(tool, cut));
    checks
}

/// Whether any finding is an error.
pub fn has_error(checks: &[ToolCheck]) -> bool {
    checks.iter().any(|c| c.severity == CheckSeverity::Error)
}

impl Tool {
    /// Get the cutting diameter of the tool.
    pub fn diameter(&self) -> f64 {
        match self {
            Tool::FlatEndMill { diameter, .. } => *diameter,
            Tool::BallEndMill { diameter, .. } => *diameter,
            Tool::BullEndMill { diameter, .. } => *diameter,
            Tool::VBit { diameter, .. } => *diameter,
            Tool::Drill { diameter, .. } => *diameter,
            Tool::FaceMill { diameter, .. } => *diameter,
        }
    }

    /// A name for the kind of tool this is, for messages.
    pub fn kind_name(&self) -> &'static str {
        match self {
            Tool::FlatEndMill { .. } => "flat end mill",
            Tool::BallEndMill { .. } => "ball end mill",
            Tool::BullEndMill { .. } => "bull end mill",
            Tool::VBit { .. } => "V-bit",
            Tool::Drill { .. } => "drill",
            Tool::FaceMill { .. } => "face mill",
        }
    }

    /// Get the tool radius.
    pub fn radius(&self) -> f64 {
        self.diameter() / 2.0
    }

    /// Get the corner radius (for bull endmills).
    pub fn corner_radius(&self) -> f64 {
        match self {
            Tool::BullEndMill { corner_radius, .. } => *corner_radius,
            Tool::BallEndMill { diameter, .. } => diameter / 2.0,
            _ => 0.0,
        }
    }

    /// Get the number of flutes/cutting edges.
    pub fn flutes(&self) -> u8 {
        match self {
            Tool::FlatEndMill { flutes, .. } => *flutes,
            Tool::BallEndMill { flutes, .. } => *flutes,
            Tool::BullEndMill { flutes, .. } => *flutes,
            Tool::VBit { .. } => 2,
            Tool::Drill { .. } => 2,
            Tool::FaceMill { inserts, .. } => *inserts,
        }
    }

    /// Get the maximum cutting depth.
    pub fn max_depth(&self) -> Option<f64> {
        match self {
            Tool::FlatEndMill { flute_length, .. } => Some(*flute_length),
            Tool::BallEndMill { flute_length, .. } => Some(*flute_length),
            Tool::BullEndMill { flute_length, .. } => Some(*flute_length),
            Tool::VBit { .. } => None,
            Tool::Drill { .. } => None,
            Tool::FaceMill { .. } => None,
        }
    }

    /// Check if the tool supports 3D drop-cutter operations.
    pub fn supports_drop_cutter(&self) -> bool {
        matches!(
            self,
            Tool::FlatEndMill { .. } | Tool::BallEndMill { .. } | Tool::BullEndMill { .. }
        )
    }

    /// Create a default flat end mill (6mm, 2 flute).
    pub fn default_endmill() -> Self {
        Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        }
    }

    /// Create a default ball end mill (6mm, 2 flute).
    pub fn default_ball() -> Self {
        Tool::BallEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        }
    }

    /// Create a default drill (3mm).
    pub fn default_drill() -> Self {
        Tool::Drill {
            diameter: 3.0,
            point_angle: 118.0,
        }
    }

    /// Create a default bull end mill (6mm, 1mm corner radius, 2 flute).
    pub fn default_bull() -> Self {
        Tool::BullEndMill {
            diameter: 6.0,
            corner_radius: 1.0,
            flute_length: 20.0,
            flutes: 2,
        }
    }
}

impl ToolHolder {
    /// Create a new tool holder.
    pub fn new(diameter: f64, length: f64) -> Self {
        Self {
            diameter,
            length,
            taper_angle: 0.0,
        }
    }

    /// Create a tool holder with taper.
    pub fn with_taper(diameter: f64, length: f64, taper_angle: f64) -> Self {
        Self {
            diameter,
            length,
            taper_angle,
        }
    }

    /// Get the radius at a given height from the tool tip.
    pub fn radius_at_height(&self, height: f64) -> f64 {
        // `taper_angle == 0.0` is intentional: a straight-sided tool is
        // configured by passing literal 0.0, not a near-zero value. The
        // comparison is against a user-set constant, not a computed number.
        if height > self.length || self.taper_angle == 0.0 {
            self.diameter / 2.0
        } else {
            let tan_half_angle = (self.taper_angle.to_radians() / 2.0).tan();
            self.diameter / 2.0 - (self.length - height) * tan_half_angle
        }
    }
}

/// A tool entry in a tool library with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolEntry {
    /// Tool number (T1, T2, etc.).
    pub number: u32,
    /// Tool name/description.
    pub name: String,
    /// The tool definition.
    pub tool: Tool,
    /// Default spindle speed (RPM).
    pub default_rpm: f64,
    /// Default feed rate (mm/min).
    pub default_feed: f64,
    /// Default plunge rate (mm/min).
    pub default_plunge: f64,
    /// The physical assembly: flutes, stickout, shank, holder.
    #[serde(default)]
    pub geometry: ToolGeometry,
}

impl ToolEntry {
    /// Create a new tool entry with defaults.
    pub fn new(number: u32, name: impl Into<String>, tool: Tool) -> Self {
        Self {
            number,
            name: name.into(),
            tool,
            default_rpm: 12000.0,
            default_feed: 1000.0,
            default_plunge: 300.0,
            geometry: ToolGeometry::default(),
        }
    }

    /// Attach the physical geometry of the assembled tool.
    pub fn with_geometry(mut self, geometry: ToolGeometry) -> Self {
        self.geometry = geometry;
        self
    }

    /// Run the tool-geometry checks for a cut with this tool.
    pub fn check_cut(&self, cut: &CutContext) -> Vec<ToolCheck> {
        check_tool_for_cut(&self.tool, &self.geometry, cut)
    }
}

/// A collection of tools available for a job.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolLibrary {
    /// The tools in this library.
    pub tools: Vec<ToolEntry>,
}

impl ToolLibrary {
    /// Create a new empty tool library.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a tool to the library.
    pub fn add(&mut self, entry: ToolEntry) {
        self.tools.push(entry);
    }

    /// Get a tool by its number.
    pub fn get_by_number(&self, number: u32) -> Option<&ToolEntry> {
        self.tools.iter().find(|t| t.number == number)
    }

    /// Get a tool by index.
    pub fn get(&self, index: usize) -> Option<&ToolEntry> {
        self.tools.get(index)
    }

    /// Create a default library with common tools.
    pub fn default_library() -> Self {
        let mut lib = Self::new();
        lib.add(ToolEntry::new(
            1,
            "6mm Flat Endmill",
            Tool::default_endmill(),
        ));
        lib.add(ToolEntry::new(2, "6mm Ball Endmill", Tool::default_ball()));
        lib.add(ToolEntry::new(
            3,
            "6mm Bull Endmill R1",
            Tool::default_bull(),
        ));
        // The drill's flute length lives in the geometry: `Tool::Drill` has no
        // field for it, and a drill op refuses a depth it cannot check.
        lib.add(
            ToolEntry::new(4, "3mm Drill", Tool::default_drill())
                .with_geometry(ToolGeometry::new().with_flute_length(30.0)),
        );
        lib.add(ToolEntry::new(
            5,
            "90° V-Bit",
            Tool::VBit {
                diameter: 6.0,
                angle: 90.0,
            },
        ));
        lib
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_diameter() {
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        assert!((tool.diameter() - 6.0).abs() < 1e-6);
        assert!((tool.radius() - 3.0).abs() < 1e-6);
    }

    #[test]
    fn test_bull_endmill() {
        let tool = Tool::BullEndMill {
            diameter: 10.0,
            corner_radius: 2.0,
            flute_length: 25.0,
            flutes: 4,
        };
        assert!((tool.diameter() - 10.0).abs() < 1e-6);
        assert!((tool.corner_radius() - 2.0).abs() < 1e-6);
        assert!(tool.supports_drop_cutter());
    }

    #[test]
    fn test_tool_serialization() {
        let tool = Tool::FlatEndMill {
            diameter: 6.0,
            flute_length: 20.0,
            flutes: 2,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("FlatEndMill"));
        let parsed: Tool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tool);
    }

    #[test]
    fn test_bull_endmill_serialization() {
        let tool = Tool::BullEndMill {
            diameter: 10.0,
            corner_radius: 2.0,
            flute_length: 25.0,
            flutes: 4,
        };
        let json = serde_json::to_string(&tool).unwrap();
        assert!(json.contains("BullEndMill"));
        let parsed: Tool = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, tool);
    }

    #[test]
    fn test_tool_holder() {
        let holder = ToolHolder::new(20.0, 50.0);
        assert!((holder.diameter - 20.0).abs() < 1e-6);
        assert!((holder.radius_at_height(30.0) - 10.0).abs() < 1e-6);

        let tapered = ToolHolder::with_taper(20.0, 50.0, 10.0);
        assert!(tapered.radius_at_height(0.0) < tapered.radius_at_height(50.0));
    }

    #[test]
    fn test_tool_entry() {
        let entry = ToolEntry::new(1, "Test Endmill", Tool::default_endmill());
        assert_eq!(entry.number, 1);
        assert_eq!(entry.name, "Test Endmill");
    }

    #[test]
    fn test_tool_library() {
        let lib = ToolLibrary::default_library();
        assert_eq!(lib.tools.len(), 5);
        assert!(lib.get_by_number(1).is_some());
        assert!(lib.get_by_number(99).is_none());
    }

    fn endmill(diameter: f64, flute_length: f64) -> Tool {
        Tool::FlatEndMill {
            diameter,
            flute_length,
            flutes: 2,
        }
    }

    /// A library written before `ToolGeometry` and `taper_angle` existed still
    /// loads, with every unknown left unknown.
    #[test]
    fn test_tool_entry_loads_json_without_geometry() {
        let json = r#"{"number":7,"name":"Ø2 2F","tool":{"type":"FlatEndMill","diameter":2.0,"flute_length":8.0,"flutes":2},"default_rpm":12000.0,"default_feed":250.0,"default_plunge":40.0}"#;
        let entry: ToolEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.number, 7);
        assert_eq!(entry.geometry, ToolGeometry::default());
        assert_eq!(entry.geometry.stickout, None);
        // The tool's own flute length still answers the question.
        assert_eq!(entry.geometry.flute_length_of(&entry.tool), Some(8.0));

        let holder: ToolHolder =
            serde_json::from_str(r#"{"diameter":20.0,"length":50.0}"#).unwrap();
        assert!((holder.taper_angle - 0.0).abs() < 1e-12);
    }

    /// The flute-length error fires exactly at the flute length, not before.
    #[test]
    fn test_check_flute_length_threshold() {
        let tool = endmill(6.0, 20.0);
        let geom = ToolGeometry::new();

        let ok = check_flute_length(&tool, &geom, &CutContext::new(20.0));
        assert!(ok.is_empty(), "{ok:?}");

        let over = check_flute_length(&tool, &geom, &CutContext::new(20.001));
        assert!(has_error(&over), "{over:?}");
        let depths: Vec<_> = over
            .iter()
            .map(|c| (c.severity, c.kind.clone()))
            .filter(|(_, k)| matches!(k, ToolCheckKind::DepthBeyondFluteLength { .. }))
            .collect();
        assert_eq!(depths.len(), 1, "{over:?}");
        assert!(matches!(
            depths[0].1,
            ToolCheckKind::DepthBeyondFluteLength { depth, flute_length }
                if (depth - 20.001).abs() < 1e-9 && (flute_length - 20.0).abs() < 1e-9
        ));
    }

    /// Past the flutes a plain shank rubs (error); a necked one only warns,
    /// and reports the clearance it actually has.
    #[test]
    fn test_check_shank_rub_depends_on_neck() {
        let tool = endmill(6.0, 10.0);
        let cut = CutContext::new(12.0);

        let plain = check_flute_length(&tool, &ToolGeometry::new(), &cut);
        let rub = plain
            .iter()
            .find(|c| matches!(c.kind, ToolCheckKind::ShankRub { .. }))
            .unwrap();
        assert_eq!(rub.severity, CheckSeverity::Error);

        let necked = check_flute_length(&tool, &ToolGeometry::new().with_shank_diameter(5.0), &cut);
        let rub = necked
            .iter()
            .find(|c| matches!(c.kind, ToolCheckKind::ShankRub { .. }))
            .unwrap();
        assert_eq!(rub.severity, CheckSeverity::Warning);
        assert!(
            rub.message.contains("0.50 mm clearance per side"),
            "{rub:?}"
        );
        // The depth error stands either way.
        assert!(has_error(&necked));
    }

    /// Stickout must cover the cut plus the holder clearance, to the
    /// millimetre.
    #[test]
    fn test_check_stickout_threshold() {
        let tool = endmill(6.0, 25.0);
        let geom = ToolGeometry::new()
            .with_stickout(12.0)
            .with_holder(ToolHolder::new(20.0, 50.0));

        // 10 mm deep + 2 mm clearance = exactly the 12 mm stickout.
        assert!(check_stickout(&tool, &geom, &CutContext::new(10.0)).is_empty());

        let over = check_stickout(&tool, &geom, &CutContext::new(10.001));
        assert!(has_error(&over), "{over:?}");
        assert!(matches!(
            over[0].kind,
            ToolCheckKind::HolderIntoStock { stickout, clearance, .. }
                if (stickout - 12.0).abs() < 1e-9 && (clearance - 2.0).abs() < 1e-9
        ));

        // A bigger clearance requirement moves the threshold with it.
        let tight = CutContext::new(10.0).with_holder_clearance(3.0);
        assert!(has_error(&check_stickout(&tool, &geom, &tight)));

        // Undeclared stickout is a warning that says so, never silence.
        let unknown = check_stickout(&tool, &ToolGeometry::new(), &CutContext::new(10.0));
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].severity, CheckSeverity::Warning);
        assert_eq!(unknown[0].kind, ToolCheckKind::StickoutUnknown);
    }

    /// Small cutters warn at 3×D, bigger ones at 5×D, and only when slotting.
    #[test]
    fn test_check_slot_ratio_thresholds() {
        let small = endmill(2.0, 12.0);
        assert!(check_slot_ratio(&small, &CutContext::new(6.0).slotting()).is_empty());
        let over = check_slot_ratio(&small, &CutContext::new(6.02).slotting());
        assert_eq!(over.len(), 1, "{over:?}");
        assert_eq!(over[0].severity, CheckSeverity::Warning);
        assert!(matches!(
            over[0].kind,
            ToolCheckKind::SlenderSlot { threshold, .. } if (threshold - 3.0).abs() < 1e-9
        ));
        // Side cutting at the same depth is not a slot.
        assert!(check_slot_ratio(&small, &CutContext::new(6.02)).is_empty());

        let big = endmill(6.0, 40.0);
        assert!(check_slot_ratio(&big, &CutContext::new(30.0).slotting()).is_empty());
        let over = check_slot_ratio(&big, &CutContext::new(30.06).slotting());
        assert_eq!(over.len(), 1, "{over:?}");
        assert!(matches!(
            over[0].kind,
            ToolCheckKind::SlenderSlot { threshold, .. } if (threshold - 5.0).abs() < 1e-9
        ));
    }

    /// Centre-cutting is answered by the tool type where the type settles it,
    /// left open where it does not, and always overridden by a declaration.
    #[test]
    fn test_centre_cutting_fallbacks() {
        let geom = ToolGeometry::new();
        assert_eq!(geom.centre_cutting_of(&Tool::default_drill()), Some(true));
        assert_eq!(geom.centre_cutting_of(&Tool::default_ball()), Some(true));
        assert_eq!(geom.centre_cutting_of(&endmill(6.0, 20.0)), None);
        assert_eq!(geom.centre_cutting_of(&Tool::default_bull()), None);
        assert_eq!(
            geom.centre_cutting_of(&Tool::FaceMill {
                diameter: 50.0,
                inserts: 4
            }),
            Some(false)
        );
        let declared = ToolGeometry::new().with_centre_cutting(true);
        assert_eq!(declared.centre_cutting_of(&endmill(6.0, 20.0)), Some(true));
        let declared = ToolGeometry::new().with_centre_cutting(false);
        assert_eq!(
            declared.centre_cutting_of(&Tool::default_drill()),
            Some(false)
        );
    }
}
