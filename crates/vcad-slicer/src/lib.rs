#![warn(missing_docs)]

//! 3D printing slicer for the vcad kernel.
//!
//! This crate provides mesh slicing, perimeter generation, and infill
//! algorithms for converting triangle meshes into toolpaths for 3D printing.
//!
//! # Example
//!
//! ```ignore
//! use vcad_slicer::{SliceSettings, slice};
//! use vcad_kernel_tessellate::TriangleMesh;
//!
//! let mesh: TriangleMesh = // ... get mesh from BRep
//! let settings = SliceSettings::default();
//! let result = slice(&mesh, &settings)?;
//!
//! println!("Layers: {}", result.layers.len());
//! println!("Print time: {:.0}s", result.stats.print_time_seconds);
//! ```

pub mod error;
pub mod infill;
pub mod path;
pub mod perimeter;
pub mod slice;
pub mod support;

#[cfg(feature = "analysis")]
pub mod analyze;
pub mod cost;
#[cfg(feature = "analysis")]
pub mod dfm;
#[cfg(feature = "analysis")]
pub mod smart_defaults;

pub use error::{Result, SlicerError};
pub use infill::{generate_infill, InfillPattern, InfillResult, InfillSettings};
pub use path::{Polygon, Polyline};
pub use perimeter::{generate_perimeters, LayerPerimeters, PerimeterSettings};
pub use slice::{generate_layer_heights, mesh_bounds, slice_mesh, SliceLayer};
pub use support::{detect_overhangs, LayerSupport, SupportSettings};

use serde::{Deserialize, Serialize};
use vcad_kernel_tessellate::TriangleMesh;

/// Slicing parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SliceSettings {
    /// Layer height for non-first layers (mm).
    pub layer_height: f64,
    /// First layer height (mm).
    pub first_layer_height: f64,
    /// Nozzle diameter (mm).
    pub nozzle_diameter: f64,
    /// Extrusion line width (mm).
    pub line_width: f64,
    /// Number of perimeter walls.
    pub wall_count: u32,
    /// Infill density (0.0 to 1.0).
    pub infill_density: f64,
    /// Infill pattern.
    pub infill_pattern: InfillPattern,
    /// Enable support structures.
    pub support_enabled: bool,
    /// Support overhang angle threshold (degrees).
    pub support_angle: f64,
}

impl Default for SliceSettings {
    fn default() -> Self {
        Self {
            layer_height: 0.2,
            first_layer_height: 0.25,
            nozzle_diameter: 0.4,
            line_width: 0.45,
            wall_count: 3,
            infill_density: 0.15,
            infill_pattern: InfillPattern::Grid,
            support_enabled: false,
            support_angle: 45.0,
        }
    }
}

impl SliceSettings {
    /// Validate settings.
    pub fn validate(&self) -> Result<()> {
        if self.layer_height <= 0.0 || self.layer_height > 1.0 {
            return Err(SlicerError::InvalidSettings(
                "layer_height must be between 0 and 1mm".into(),
            ));
        }
        if self.first_layer_height <= 0.0 {
            return Err(SlicerError::InvalidSettings(
                "first_layer_height must be positive".into(),
            ));
        }
        if self.line_width <= 0.0 {
            return Err(SlicerError::InvalidSettings(
                "line_width must be positive".into(),
            ));
        }
        if self.infill_density < 0.0 || self.infill_density > 1.0 {
            return Err(SlicerError::InvalidSettings(
                "infill_density must be between 0 and 1".into(),
            ));
        }
        Ok(())
    }
}

/// A complete layer ready for G-code generation.
#[derive(Debug, Clone)]
pub struct PrintLayer {
    /// Z height (mm).
    pub z: f64,
    /// Layer index.
    pub index: usize,
    /// Layer height at this position (mm).
    pub layer_height: f64,
    /// Outer perimeters (printed first, slow).
    pub outer_perimeters: Vec<Polygon>,
    /// Inner perimeters.
    pub inner_perimeters: Vec<Polygon>,
    /// Infill paths.
    pub infill: Vec<Polyline>,
    /// Support structures (if enabled).
    pub support: Option<Vec<Polygon>>,
}

/// Statistics about the sliced model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrintStats {
    /// Total number of layers.
    pub layer_count: usize,
    /// Estimated print time in seconds.
    pub print_time_seconds: f64,
    /// Total filament length in mm.
    pub filament_mm: f64,
    /// Total filament weight in grams (assuming PLA at 1.24 g/cm³).
    pub filament_grams: f64,
    /// Bounding box min corner.
    pub bounds_min: [f64; 3],
    /// Bounding box max corner.
    pub bounds_max: [f64; 3],
}

/// Result of slicing operation.
#[derive(Debug, Clone)]
pub struct SliceResult {
    /// Print layers with toolpaths.
    pub layers: Vec<PrintLayer>,
    /// Print statistics.
    pub stats: PrintStats,
}

/// Slice a mesh with the given settings.
///
/// This is the main entry point for slicing. It:
/// 1. Slices the mesh into layers
/// 2. Generates perimeters for each layer
/// 3. Generates infill
/// 4. Optionally generates support structures
/// 5. Computes print statistics
pub fn slice(mesh: &TriangleMesh, settings: &SliceSettings) -> Result<SliceResult> {
    settings.validate()?;

    // Get mesh bounds
    let (bounds_min, bounds_max) =
        mesh_bounds(mesh).ok_or(SlicerError::EmptyMesh)?;

    // Generate layer heights
    let layer_heights = generate_layer_heights(
        bounds_min[2],
        bounds_max[2],
        settings.first_layer_height,
        settings.layer_height,
    );

    if layer_heights.is_empty() {
        return Err(SlicerError::SliceFailed("model too thin to slice".into()));
    }

    // Slice mesh
    let slice_layers = slice_mesh(mesh, &layer_heights)?;

    // Detect support if enabled
    let support_layers = if settings.support_enabled {
        let support_settings = SupportSettings {
            overhang_angle: settings.support_angle,
            density: 0.15,
            ..Default::default()
        };
        Some(detect_overhangs(mesh, &slice_layers, &support_settings))
    } else {
        None
    };

    // Process each layer
    let perimeter_settings = PerimeterSettings {
        wall_count: settings.wall_count,
        line_width: settings.line_width,
        ..Default::default()
    };

    let mut print_layers: Vec<PrintLayer> = Vec::with_capacity(slice_layers.len());
    let mut total_path_length = 0.0;

    for (idx, slice_layer) in slice_layers.iter().enumerate() {
        let layer_height = if idx == 0 {
            settings.first_layer_height
        } else {
            settings.layer_height
        };

        // Generate perimeters
        let perimeters = generate_perimeters(slice_layer, &perimeter_settings);

        // Generate infill
        let infill_settings = InfillSettings {
            pattern: settings.infill_pattern,
            density: settings.infill_density,
            line_width: settings.line_width,
            layer_index: idx,
        };
        let infill = generate_infill(&perimeters.infill_boundary, &infill_settings);

        // Calculate path length for this layer
        for poly in &perimeters.outer {
            total_path_length += poly.perimeter();
        }
        for poly in &perimeters.inner {
            total_path_length += poly.perimeter();
        }
        for path in &infill.paths {
            total_path_length += path.length();
        }

        // Get support for this layer
        let support = support_layers
            .as_ref()
            .and_then(|layers| layers.get(idx))
            .filter(|s| !s.regions.is_empty())
            .map(|s| s.regions.clone());

        print_layers.push(PrintLayer {
            z: slice_layer.z,
            index: idx,
            layer_height,
            outer_perimeters: perimeters.outer,
            inner_perimeters: perimeters.inner,
            infill: infill.paths,
            support,
        });
    }

    // Compute statistics
    let _filament_mm = total_path_length;

    // Cross-sectional area of extruded filament (approximate)
    let filament_diameter: f64 = 1.75; // mm
    let nozzle_area = settings.line_width * settings.layer_height;
    let filament_area = std::f64::consts::PI * (filament_diameter / 2.0).powi(2);
    let filament_length = total_path_length * nozzle_area / filament_area;

    // Weight (PLA density ~1.24 g/cm³)
    let filament_volume_cm3 = filament_area * filament_length / 1000.0; // mm³ to cm³
    let filament_grams = filament_volume_cm3 * 1.24;

    // Print time estimate (rough: assume 60mm/s average including travel)
    let print_speed = 60.0; // mm/s
    let print_time_seconds = total_path_length / print_speed;

    let stats = PrintStats {
        layer_count: print_layers.len(),
        print_time_seconds,
        filament_mm: filament_length,
        filament_grams,
        bounds_min,
        bounds_max,
    };

    Ok(SliceResult {
        layers: print_layers,
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cube_mesh() -> TriangleMesh {
        let size = 10.0f32;
        let vertices = vec![
            0.0, 0.0, 0.0, size, 0.0, 0.0, size, size, 0.0, 0.0, size, 0.0,
            0.0, 0.0, size, size, 0.0, size, size, size, size, 0.0, size, size,
        ];
        let indices = vec![
            0, 2, 1, 0, 3, 2,
            4, 5, 6, 4, 6, 7,
            0, 1, 5, 0, 5, 4,
            2, 3, 7, 2, 7, 6,
            0, 4, 7, 0, 7, 3,
            1, 2, 6, 1, 6, 5,
        ];
        TriangleMesh {
            vertices,
            indices,
            normals: Vec::new(),
        }
    }

    #[test]
    fn test_slice_cube() {
        let mesh = make_cube_mesh();
        let settings = SliceSettings {
            layer_height: 0.5, // Large layers for fast test
            first_layer_height: 0.5,
            infill_density: 0.05, // Low density for fast test
            wall_count: 1, // Minimal walls
            ..Default::default()
        };
        let result = slice(&mesh, &settings).unwrap();

        assert!(!result.layers.is_empty());
        assert!(result.stats.layer_count > 0);
        assert!(result.stats.layer_count <= 30); // ~20 layers for 10mm cube at 0.5mm
    }

    #[test]
    fn test_invalid_settings() {
        let settings = SliceSettings {
            layer_height: -0.1,
            ..Default::default()
        };
        assert!(settings.validate().is_err());
    }
}
