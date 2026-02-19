//! Thermal analysis for PCB components and vias.
//!
//! Provides junction temperature estimation using the theta-JA thermal
//! resistance model, and via thermal resistance calculation based on
//! copper plating geometry.

use std::f64::consts::PI;

/// Thermal conductivity of copper in W/(m*K).
const K_COPPER: f64 = 385.0;

/// Component thermal specification.
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentThermal {
    /// Component reference designator (e.g. "U1", "Q3").
    pub reference: String,
    /// Power dissipation in watts.
    pub power_watts: f64,
    /// Junction-to-ambient thermal resistance in degrees C per watt.
    pub theta_ja: f64,
}

/// Thermal analysis result for a single component.
#[derive(Debug, Clone, PartialEq)]
pub struct ThermalResult {
    /// Component reference designator.
    pub reference: String,
    /// Temperature rise above ambient in degrees C.
    pub temperature_rise: f64,
    /// Estimated junction temperature in degrees C.
    pub junction_temperature: f64,
}

/// Estimate temperature rise and junction temperature for components.
///
/// Uses the simple theta-JA model: T_junction = T_ambient + P * theta_JA.
///
/// This model assumes each component dissipates heat independently. For
/// more accurate results in dense designs, a full FEA thermal simulation
/// would be needed.
///
/// # Arguments
///
/// * `components` - Slice of component thermal specifications
/// * `ambient_temp` - Ambient temperature in degrees C
///
/// # Returns
///
/// A vector of [`ThermalResult`] for each component.
pub fn analyze_thermal(components: &[ComponentThermal], ambient_temp: f64) -> Vec<ThermalResult> {
    components
        .iter()
        .map(|c| {
            let temperature_rise = c.power_watts * c.theta_ja;
            ThermalResult {
                reference: c.reference.clone(),
                temperature_rise,
                junction_temperature: ambient_temp + temperature_rise,
            }
        })
        .collect()
}

/// Estimate the thermal resistance of a plated-through via in degrees C per watt.
///
/// Models the via as a hollow copper cylinder. The thermal resistance is
/// determined by the copper plating cross-section and board thickness.
///
/// # Formula
///
/// ```text
/// R_via = board_thickness / (pi * drill_diameter * plating_thickness * k_copper)
/// ```
///
/// where `k_copper = 385 W/(m*K)`. All dimensions must be in mm; the result
/// is converted to degrees C per watt.
///
/// # Arguments
///
/// * `drill_diameter_mm` - Via drill diameter in mm
/// * `plating_thickness_mm` - Copper plating thickness in mm
/// * `board_thickness_mm` - Total board thickness in mm
///
/// # Returns
///
/// Thermal resistance in degrees C per watt.
///
/// # Panics
///
/// Panics if any input is non-positive.
pub fn via_thermal_resistance(
    drill_diameter_mm: f64,
    plating_thickness_mm: f64,
    board_thickness_mm: f64,
) -> f64 {
    assert!(drill_diameter_mm > 0.0, "drill diameter must be positive");
    assert!(
        plating_thickness_mm > 0.0,
        "plating thickness must be positive"
    );
    assert!(board_thickness_mm > 0.0, "board thickness must be positive");

    // Convert mm to meters for SI units
    let drill_m = drill_diameter_mm * 1e-3;
    let plating_m = plating_thickness_mm * 1e-3;
    let thickness_m = board_thickness_mm * 1e-3;

    // R = L / (pi * d * t * k)
    thickness_m / (PI * drill_m * plating_m * K_COPPER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_thermal_basic() {
        let components = vec![
            ComponentThermal {
                reference: "U1".to_string(),
                power_watts: 1.0,
                theta_ja: 40.0,
            },
            ComponentThermal {
                reference: "Q1".to_string(),
                power_watts: 0.5,
                theta_ja: 60.0,
            },
        ];

        let results = analyze_thermal(&components, 25.0);
        assert_eq!(results.len(), 2);

        // U1: 25 + 1.0 * 40.0 = 65 C
        assert_eq!(results[0].reference, "U1");
        assert!((results[0].temperature_rise - 40.0).abs() < 1e-10);
        assert!((results[0].junction_temperature - 65.0).abs() < 1e-10);

        // Q1: 25 + 0.5 * 60.0 = 55 C
        assert_eq!(results[1].reference, "Q1");
        assert!((results[1].temperature_rise - 30.0).abs() < 1e-10);
        assert!((results[1].junction_temperature - 55.0).abs() < 1e-10);
    }

    #[test]
    fn test_analyze_thermal_zero_power() {
        let components = vec![ComponentThermal {
            reference: "R1".to_string(),
            power_watts: 0.0,
            theta_ja: 100.0,
        }];
        let results = analyze_thermal(&components, 30.0);
        assert!((results[0].temperature_rise).abs() < 1e-10);
        assert!((results[0].junction_temperature - 30.0).abs() < 1e-10);
    }

    #[test]
    fn test_analyze_thermal_high_ambient() {
        let components = vec![ComponentThermal {
            reference: "U2".to_string(),
            power_watts: 2.0,
            theta_ja: 30.0,
        }];
        // Automotive ambient: 85 C
        let results = analyze_thermal(&components, 85.0);
        // 85 + 2.0 * 30.0 = 145 C
        assert!((results[0].junction_temperature - 145.0).abs() < 1e-10);
    }

    #[test]
    fn test_analyze_thermal_empty() {
        let results = analyze_thermal(&[], 25.0);
        assert!(results.is_empty());
    }

    #[test]
    fn test_via_thermal_resistance_typical() {
        // Typical via: 0.3mm drill, 0.025mm plating, 1.6mm board
        // R = 0.0016 / (pi * 0.0003 * 0.000025 * 385) = ~176 C/W
        let r = via_thermal_resistance(0.3, 0.025, 1.6);

        // Manual calculation:
        let expected = 0.0016 / (PI * 0.0003 * 0.000025 * 385.0);
        assert!(
            (r - expected).abs() < 0.1,
            "expected ~{expected:.1} C/W, got {r:.1} C/W"
        );
        // Sanity check: should be in the range ~100-300 C/W for a typical via
        assert!(r > 50.0, "via thermal resistance should be > 50 C/W");
        assert!(r < 500.0, "via thermal resistance should be < 500 C/W");
    }

    #[test]
    fn test_via_thermal_resistance_larger_via_lower_resistance() {
        let small = via_thermal_resistance(0.2, 0.025, 1.6);
        let large = via_thermal_resistance(0.5, 0.025, 1.6);
        assert!(
            large < small,
            "larger drill diameter should give lower thermal resistance"
        );
    }

    #[test]
    fn test_via_thermal_resistance_thicker_plating_lower_resistance() {
        let thin = via_thermal_resistance(0.3, 0.015, 1.6);
        let thick = via_thermal_resistance(0.3, 0.050, 1.6);
        assert!(
            thick < thin,
            "thicker plating should give lower thermal resistance"
        );
    }

    #[test]
    fn test_via_thermal_resistance_thicker_board_higher_resistance() {
        let thin_board = via_thermal_resistance(0.3, 0.025, 0.8);
        let thick_board = via_thermal_resistance(0.3, 0.025, 2.4);
        assert!(
            thick_board > thin_board,
            "thicker board should give higher thermal resistance"
        );
    }

    #[test]
    fn test_via_thermal_resistance_scales_linearly_with_thickness() {
        let r1 = via_thermal_resistance(0.3, 0.025, 1.0);
        let r2 = via_thermal_resistance(0.3, 0.025, 2.0);
        assert!(
            (r2 / r1 - 2.0).abs() < 1e-10,
            "resistance should scale linearly with board thickness"
        );
    }

    #[test]
    fn test_via_thermal_resistance_array() {
        // Multiple vias in parallel: total R = R_single / N
        let single = via_thermal_resistance(0.3, 0.025, 1.6);
        let array_4 = single / 4.0;
        // 4 vias should have ~1/4 the thermal resistance
        assert!(array_4 < single);
        assert!((array_4 * 4.0 - single).abs() < 1e-10);
    }

    #[test]
    #[should_panic(expected = "drill diameter must be positive")]
    fn test_via_thermal_resistance_zero_drill() {
        via_thermal_resistance(0.0, 0.025, 1.6);
    }

    #[test]
    #[should_panic(expected = "plating thickness must be positive")]
    fn test_via_thermal_resistance_zero_plating() {
        via_thermal_resistance(0.3, 0.0, 1.6);
    }

    #[test]
    #[should_panic(expected = "board thickness must be positive")]
    fn test_via_thermal_resistance_zero_board() {
        via_thermal_resistance(0.3, 0.025, 0.0);
    }
}
