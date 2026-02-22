//! Printer profile definitions.

use serde::{Deserialize, Serialize};

use crate::flavor::GcodeFlavor;

/// Printer profile with machine-specific settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrinterProfile {
    /// Profile name.
    pub name: String,
    /// G-code flavor.
    pub flavor: GcodeFlavor,
    /// Build volume X (mm).
    pub bed_x: f64,
    /// Build volume Y (mm).
    pub bed_y: f64,
    /// Build volume Z (mm).
    pub bed_z: f64,
    /// Is the bed heated?
    pub heated_bed: bool,
    /// Number of extruders.
    pub extruder_count: u32,
    /// Nozzle diameter (mm).
    pub nozzle_diameter: f64,
    /// Filament diameter (mm).
    pub filament_diameter: f64,
    /// Maximum feedrate X (mm/s).
    pub max_feedrate_x: f64,
    /// Maximum feedrate Y (mm/s).
    pub max_feedrate_y: f64,
    /// Maximum feedrate Z (mm/s).
    pub max_feedrate_z: f64,
    /// Maximum feedrate E (mm/s).
    pub max_feedrate_e: f64,
    /// Maximum acceleration (mm/s²).
    pub max_acceleration: f64,
    /// Default print temperature (°C).
    pub default_print_temp: u32,
    /// Default bed temperature (°C).
    pub default_bed_temp: u32,
    /// Retraction distance (mm).
    pub retraction_distance: f64,
    /// Retraction speed (mm/s).
    pub retraction_speed: f64,
    /// Z-hop height during retraction (mm).
    pub z_hop: f64,
}

impl Default for PrinterProfile {
    fn default() -> Self {
        Self::generic()
    }
}

impl PrinterProfile {
    /// Generic printer profile.
    pub fn generic() -> Self {
        Self {
            name: "Generic".into(),
            flavor: GcodeFlavor::Marlin,
            bed_x: 220.0,
            bed_y: 220.0,
            bed_z: 250.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 10.0,
            max_feedrate_e: 60.0,
            max_acceleration: 3000.0,
            default_print_temp: 210,
            default_bed_temp: 60,
            retraction_distance: 5.0,
            retraction_speed: 45.0,
            z_hop: 0.2,
        }
    }

    /// Bambu Lab X1 Carbon profile.
    pub fn bambu_x1c() -> Self {
        Self {
            name: "Bambu Lab X1 Carbon".into(),
            flavor: GcodeFlavor::Bambu,
            bed_x: 256.0,
            bed_y: 256.0,
            bed_z: 256.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 20.0,
            max_feedrate_e: 80.0,
            max_acceleration: 10000.0,
            default_print_temp: 220,
            default_bed_temp: 55,
            retraction_distance: 0.8,
            retraction_speed: 30.0,
            z_hop: 0.4,
        }
    }

    /// Bambu Lab P1S profile.
    pub fn bambu_p1s() -> Self {
        Self {
            name: "Bambu Lab P1S".into(),
            flavor: GcodeFlavor::Bambu,
            bed_x: 256.0,
            bed_y: 256.0,
            bed_z: 256.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 20.0,
            max_feedrate_e: 80.0,
            max_acceleration: 10000.0,
            default_print_temp: 220,
            default_bed_temp: 55,
            retraction_distance: 0.8,
            retraction_speed: 30.0,
            z_hop: 0.4,
        }
    }

    /// Bambu Lab A1 profile.
    pub fn bambu_a1() -> Self {
        Self {
            name: "Bambu Lab A1".into(),
            flavor: GcodeFlavor::Bambu,
            bed_x: 256.0,
            bed_y: 256.0,
            bed_z: 256.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 16.0,
            max_feedrate_e: 60.0,
            max_acceleration: 10000.0,
            default_print_temp: 220,
            default_bed_temp: 55,
            retraction_distance: 0.8,
            retraction_speed: 30.0,
            z_hop: 0.4,
        }
    }

    /// Bambu Lab A1 Mini profile.
    pub fn bambu_a1_mini() -> Self {
        Self {
            name: "Bambu Lab A1 Mini".into(),
            flavor: GcodeFlavor::Bambu,
            bed_x: 180.0,
            bed_y: 180.0,
            bed_z: 180.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 16.0,
            max_feedrate_e: 60.0,
            max_acceleration: 10000.0,
            default_print_temp: 220,
            default_bed_temp: 55,
            retraction_distance: 0.8,
            retraction_speed: 30.0,
            z_hop: 0.4,
        }
    }

    /// Creality Ender 3 profile.
    pub fn ender3() -> Self {
        Self {
            name: "Creality Ender 3".into(),
            flavor: GcodeFlavor::Marlin,
            bed_x: 220.0,
            bed_y: 220.0,
            bed_z: 250.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 500.0,
            max_feedrate_y: 500.0,
            max_feedrate_z: 5.0,
            max_feedrate_e: 25.0,
            max_acceleration: 500.0,
            default_print_temp: 200,
            default_bed_temp: 60,
            retraction_distance: 5.0,
            retraction_speed: 45.0,
            z_hop: 0.2,
        }
    }

    /// Prusa MK4 profile.
    pub fn prusa_mk4() -> Self {
        Self {
            name: "Prusa MK4".into(),
            flavor: GcodeFlavor::Marlin,
            bed_x: 250.0,
            bed_y: 210.0,
            bed_z: 220.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 200.0,
            max_feedrate_y: 200.0,
            max_feedrate_z: 12.0,
            max_feedrate_e: 120.0,
            max_acceleration: 4000.0,
            default_print_temp: 215,
            default_bed_temp: 60,
            retraction_distance: 0.8,
            retraction_speed: 35.0,
            z_hop: 0.2,
        }
    }

    /// Voron 2.4 profile (Klipper).
    pub fn voron_24() -> Self {
        Self {
            name: "Voron 2.4 (350mm)".into(),
            flavor: GcodeFlavor::Klipper,
            bed_x: 350.0,
            bed_y: 350.0,
            bed_z: 340.0,
            heated_bed: true,
            extruder_count: 1,
            nozzle_diameter: 0.4,
            filament_diameter: 1.75,
            max_feedrate_x: 300.0,
            max_feedrate_y: 300.0,
            max_feedrate_z: 15.0,
            max_feedrate_e: 60.0,
            max_acceleration: 5000.0,
            default_print_temp: 240,
            default_bed_temp: 110,
            retraction_distance: 0.5,
            retraction_speed: 30.0,
            z_hop: 0.2,
        }
    }

    /// Get all built-in profiles.
    pub fn all_profiles() -> Vec<Self> {
        vec![
            Self::generic(),
            Self::bambu_x1c(),
            Self::bambu_p1s(),
            Self::bambu_a1(),
            Self::bambu_a1_mini(),
            Self::ender3(),
            Self::prusa_mk4(),
            Self::voron_24(),
        ]
    }

    /// Check if a position is within build volume.
    pub fn in_bounds(&self, x: f64, y: f64, z: f64) -> bool {
        x >= 0.0 && x <= self.bed_x && y >= 0.0 && y <= self.bed_y && z >= 0.0 && z <= self.bed_z
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_profiles() {
        for profile in PrinterProfile::all_profiles() {
            assert!(profile.bed_x > 0.0);
            assert!(profile.bed_y > 0.0);
            assert!(profile.bed_z > 0.0);
            assert!(profile.nozzle_diameter > 0.0);
        }
    }

    #[test]
    fn test_in_bounds() {
        let profile = PrinterProfile::bambu_x1c();
        assert!(profile.in_bounds(100.0, 100.0, 100.0));
        assert!(!profile.in_bounds(-1.0, 100.0, 100.0));
        assert!(!profile.in_bounds(300.0, 100.0, 100.0));
    }
}
