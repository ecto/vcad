//! Hoop sizes and machine profiles.

use serde::{Deserialize, Serialize};

/// An embroidery hoop defining the stitchable area.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hoop {
    /// Hoop name (e.g. "4x4", "5x7").
    pub name: String,
    /// Stitchable width in mm.
    pub width: f64,
    /// Stitchable height in mm.
    pub height: f64,
}

/// A machine profile describing an embroidery machine's capabilities.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MachineProfile {
    /// Machine model name.
    pub name: String,
    /// Manufacturer.
    pub manufacturer: String,
    /// Available hoops.
    pub hoops: Vec<Hoop>,
    /// Maximum stitch count.
    pub max_stitches: u32,
    /// Maximum number of thread colors.
    pub max_colors: u32,
    /// Supported file format extensions.
    pub supported_formats: Vec<String>,
}

/// Brother PE800 profile.
pub fn brother_pe800() -> MachineProfile {
    MachineProfile {
        name: "PE800".into(),
        manufacturer: "Brother".into(),
        hoops: vec![Hoop {
            name: "5x7".into(),
            width: 130.0,
            height: 180.0,
        }],
        max_stitches: 300_000,
        max_colors: 127,
        supported_formats: vec!["pes".into(), "dst".into(), "phc".into()],
    }
}

/// Brother SE1900 profile.
pub fn brother_se1900() -> MachineProfile {
    MachineProfile {
        name: "SE1900".into(),
        manufacturer: "Brother".into(),
        hoops: vec![
            Hoop {
                name: "4x4".into(),
                width: 100.0,
                height: 100.0,
            },
            Hoop {
                name: "5x7".into(),
                width: 130.0,
                height: 180.0,
            },
        ],
        max_stitches: 300_000,
        max_colors: 127,
        supported_formats: vec!["pes".into(), "dst".into(), "phc".into()],
    }
}

impl Hoop {
    /// Check whether a bounding box fits within this hoop.
    ///
    /// `bounds` is `(min_x, min_y, max_x, max_y)` in mm.
    pub fn contains(&self, bounds: (f64, f64, f64, f64)) -> bool {
        let (min_x, min_y, max_x, max_y) = bounds;
        let w = max_x - min_x;
        let h = max_y - min_y;
        w <= self.width && h <= self.height
    }
}
