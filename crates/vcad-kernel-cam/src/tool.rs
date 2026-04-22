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
    pub taper_angle: f64,
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
        }
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
        lib.add(ToolEntry::new(4, "3mm Drill", Tool::default_drill()));
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
}
