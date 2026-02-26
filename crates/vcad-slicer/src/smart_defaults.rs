//! Geometry-aware smart defaults for print settings.
//!
//! Uses BRep analysis results to recommend optimal slicer settings.
//! This is where vcad's design-awareness pays off — settings are chosen
//! based on actual wall thicknesses, overhang angles, and feature sizes,
//! not user guesswork.

use serde::{Deserialize, Serialize};

use crate::analyze::PrintAnalysis;
use crate::SliceSettings;

/// Printer parameters needed for smart defaults (avoids cyclic dep on vcad-slicer-gcode).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrinterParams {
    /// Nozzle diameter (mm).
    pub nozzle_diameter: f64,
    /// Build volume X (mm).
    pub bed_x: f64,
    /// Build volume Y (mm).
    pub bed_y: f64,
    /// Build volume Z (mm).
    pub bed_z: f64,
}

impl Default for PrinterParams {
    fn default() -> Self {
        Self {
            nozzle_diameter: 0.4,
            bed_x: 220.0,
            bed_y: 220.0,
            bed_z: 250.0,
        }
    }
}

/// A recommended setting with human-readable reasoning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingRecommendation {
    /// Setting name (e.g., "wall_count", "layer_height").
    pub setting: String,
    /// Recommended value as string.
    pub value: String,
    /// Human-readable reason for the recommendation.
    pub reason: String,
}

/// Result of smart defaults recommendation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SmartDefaults {
    /// Recommended settings.
    pub settings: SliceSettings,
    /// Explanations for each recommendation.
    pub recommendations: Vec<SettingRecommendation>,
}

/// Recommend print settings based on BRep analysis and printer parameters.
///
/// Returns settings + human-readable explanations for each choice.
pub fn recommend_settings(analysis: &PrintAnalysis, params: &PrinterParams) -> SmartDefaults {
    let nozzle = params.nozzle_diameter;
    let line_width = nozzle * 1.125; // Standard 112.5% of nozzle
    let mut recommendations = Vec::new();

    // Layer height
    let layer_height = if let Some(min_feature) = analysis.min_feature_size {
        if min_feature < 1.0 {
            recommendations.push(SettingRecommendation {
                setting: "layer_height".into(),
                value: "0.12".into(),
                reason: format!("Fine layers for {:.2}mm feature detail", min_feature),
            });
            0.12
        } else if min_feature < 2.0 {
            recommendations.push(SettingRecommendation {
                setting: "layer_height".into(),
                value: "0.16".into(),
                reason: format!("Medium layers for {:.2}mm features", min_feature),
            });
            0.16
        } else {
            recommendations.push(SettingRecommendation {
                setting: "layer_height".into(),
                value: "0.20".into(),
                reason: "Standard layer height (no small features)".into(),
            });
            0.20
        }
    } else {
        0.20
    };

    // Wall count from wall thickness
    let wall_count = if let Some(min_wall) = analysis.min_wall_thickness {
        let needed = (min_wall / line_width).ceil() as u32;
        let count = needed.clamp(2, 8);
        recommendations.push(SettingRecommendation {
            setting: "wall_count".into(),
            value: count.to_string(),
            reason: format!(
                "{} walls for {:.2}mm wall thickness ({:.2}mm line width)",
                count, min_wall, line_width
            ),
        });
        count
    } else {
        recommendations.push(SettingRecommendation {
            setting: "wall_count".into(),
            value: "3".into(),
            reason: "Default wall count (wall thickness not detected)".into(),
        });
        3
    };

    // Infill density
    let infill_density = if let Some(min_wall) = analysis.min_wall_thickness {
        if min_wall < 2.0 {
            recommendations.push(SettingRecommendation {
                setting: "infill_density".into(),
                value: "30%".into(),
                reason: format!("Higher infill for thin-walled part ({:.2}mm)", min_wall),
            });
            0.30
        } else {
            recommendations.push(SettingRecommendation {
                setting: "infill_density".into(),
                value: "15%".into(),
                reason: "Standard infill density".into(),
            });
            0.15
        }
    } else {
        0.15
    };

    // Support
    let support_enabled = analysis.needs_support;
    let support_angle = if analysis.max_overhang_angle > 0.0 {
        let threshold = (analysis.max_overhang_angle - 5.0).clamp(30.0, 60.0);
        if support_enabled {
            recommendations.push(SettingRecommendation {
                setting: "support_enabled".into(),
                value: "true".into(),
                reason: format!(
                    "Support enabled ({} face(s) with up to {:.0}° overhang)",
                    analysis.overhang_faces.len(),
                    analysis.max_overhang_angle
                ),
            });
            recommendations.push(SettingRecommendation {
                setting: "support_angle".into(),
                value: format!("{:.0}°", threshold),
                reason: "Support angle from overhang analysis".into(),
            });
        }
        threshold
    } else {
        if !support_enabled {
            recommendations.push(SettingRecommendation {
                setting: "support_enabled".into(),
                value: "false".into(),
                reason: "No overhangs detected".into(),
            });
        }
        45.0
    };

    // Check if part fits build volume
    if analysis.bbox_size[0] > params.bed_x
        || analysis.bbox_size[1] > params.bed_y
        || analysis.bbox_size[2] > params.bed_z
    {
        recommendations.push(SettingRecommendation {
            setting: "build_volume".into(),
            value: "warning".into(),
            reason: format!(
                "Part ({:.0}x{:.0}x{:.0}mm) exceeds build volume ({:.0}x{:.0}x{:.0}mm)",
                analysis.bbox_size[0],
                analysis.bbox_size[1],
                analysis.bbox_size[2],
                params.bed_x,
                params.bed_y,
                params.bed_z
            ),
        });
    }

    let settings = SliceSettings {
        layer_height,
        first_layer_height: (layer_height * 1.25).min(0.3),
        nozzle_diameter: nozzle,
        line_width,
        wall_count,
        infill_density,
        infill_pattern: crate::InfillPattern::Grid,
        support_enabled,
        support_angle,
    };

    SmartDefaults {
        settings,
        recommendations,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::PrintAnalysis;

    fn default_analysis() -> PrintAnalysis {
        PrintAnalysis {
            min_wall_thickness: Some(1.6),
            overhang_faces: Vec::new(),
            max_overhang_angle: 0.0,
            holes: Vec::new(),
            min_feature_size: Some(1.6),
            volume_mm3: 4000.0,
            surface_area_mm2: 1600.0,
            bbox_size: [20.0, 20.0, 10.0],
            bridges: Vec::new(),
            needs_support: false,
            suggested_orientation: [0.0, 0.0, 0.0],
            notes: Vec::new(),
        }
    }

    #[test]
    fn test_recommend_basic() {
        let analysis = default_analysis();
        let params = PrinterParams {
            nozzle_diameter: 0.4,
            bed_x: 180.0,
            bed_y: 180.0,
            bed_z: 180.0,
        };
        let result = recommend_settings(&analysis, &params);

        assert!(result.settings.layer_height > 0.0);
        assert!(result.settings.wall_count >= 2);
        assert!(!result.recommendations.is_empty());
    }

    #[test]
    fn test_thin_wall_increases_walls() {
        let mut analysis = default_analysis();
        analysis.min_wall_thickness = Some(0.8);
        let params = PrinterParams::default();
        let result = recommend_settings(&analysis, &params);

        assert!(result.settings.wall_count >= 2);
    }

    #[test]
    fn test_overhang_enables_support() {
        let mut analysis = default_analysis();
        analysis.needs_support = true;
        analysis.max_overhang_angle = 60.0;
        analysis.overhang_faces = vec![crate::analyze::OverhangFace {
            face_index: 0,
            angle_deg: 60.0,
        }];

        let params = PrinterParams::default();
        let result = recommend_settings(&analysis, &params);

        assert!(result.settings.support_enabled);
    }
}
