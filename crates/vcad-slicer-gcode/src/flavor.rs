//! G-code flavor definitions.

use serde::{Deserialize, Serialize};

/// G-code flavor (dialect).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GcodeFlavor {
    /// Marlin firmware (Ender, Prusa).
    #[default]
    Marlin,
    /// Klipper firmware.
    Klipper,
    /// Bambu Lab printers.
    Bambu,
    /// RepRap firmware.
    RepRap,
}

impl GcodeFlavor {
    /// Get the start G-code template for this flavor.
    pub fn start_gcode(&self) -> &'static str {
        match self {
            GcodeFlavor::Marlin => {
                "G28 ; Home all axes\n\
                 G29 ; Auto bed leveling\n\
                 M104 S{print_temp} ; Set nozzle temp\n\
                 M140 S{bed_temp} ; Set bed temp\n\
                 M109 S{print_temp} ; Wait for nozzle temp\n\
                 M190 S{bed_temp} ; Wait for bed temp\n\
                 G92 E0 ; Reset extruder\n\
                 G1 Z5 F3000 ; Move Z up\n"
            }
            GcodeFlavor::Klipper => {
                "G28 ; Home all axes\n\
                 BED_MESH_CALIBRATE ; Bed mesh\n\
                 M104 S{print_temp} ; Set nozzle temp\n\
                 M140 S{bed_temp} ; Set bed temp\n\
                 M109 S{print_temp} ; Wait for nozzle temp\n\
                 M190 S{bed_temp} ; Wait for bed temp\n\
                 G92 E0 ; Reset extruder\n"
            }
            GcodeFlavor::Bambu => {
                // Written from scratch for P1/X1 (CoreXY, 256 mm bed, Z homed
                // by the nozzle touching the plate) — not Bambu's AGPL start
                // G-code. Order matters: the nozzle homes and probes at 140 °C
                // (below the ooze point), and only reaches print temperature
                // parked above the plate, right before the purge line.
                "; vcad start sequence (P1/X1)\n\
                 M400\n\
                 G90 ; absolute XYZ\n\
                 M82 ; absolute E\n\
                 M106 P1 S0 ; part fan off\n\
                 M140 S{bed_temp} ; bed heating, no wait\n\
                 M104 S140 ; nozzle to a non-oozing homing temp\n\
                 G28 ; home all (Z by nozzle touch)\n\
                 M190 S{bed_temp} ; wait for bed\n\
                 G29 ; bed mesh at temperature\n\
                 G1 Z5 F1200\n\
                 G1 X18 Y2 F12000 ; park over the purge line start\n\
                 M109 S{print_temp} ; wait for nozzle\n\
                 G92 E0\n\
                 G1 Z0.3 F600\n\
                 G1 X118 Y2 E12 F1500 ; purge line\n\
                 G1 X148 Y2 E13.5 F3000 ; thin tail\n\
                 G92 E0\n\
                 G1 Z1 F600\n"
            }
            GcodeFlavor::RepRap => {
                "G28 ; Home\n\
                 G29 ; Probe bed\n\
                 M104 S{print_temp}\n\
                 M140 S{bed_temp}\n\
                 M109 S{print_temp}\n\
                 M190 S{bed_temp}\n\
                 G92 E0\n"
            }
        }
    }

    /// Get the end G-code template for this flavor.
    pub fn end_gcode(&self) -> &'static str {
        match self {
            GcodeFlavor::Marlin => {
                "M104 S0 ; Turn off nozzle\n\
                 M140 S0 ; Turn off bed\n\
                 G91 ; Relative positioning\n\
                 G1 E-2 F2700 ; Retract\n\
                 G1 Z10 F3000 ; Move Z up\n\
                 G90 ; Absolute positioning\n\
                 G1 X0 Y200 F3000 ; Present print\n\
                 M84 ; Disable motors\n"
            }
            GcodeFlavor::Klipper => {
                "TURN_OFF_HEATERS\n\
                 G91\n\
                 G1 E-2 F2700\n\
                 G1 Z10 F3000\n\
                 G90\n\
                 G1 X0 Y200 F3000\n\
                 M84\n"
            }
            GcodeFlavor::Bambu => {
                "M400 ; Wait for moves\n\
                 M106 P1 S0 ; part fan off\n\
                 M104 S0 ; Nozzle off\n\
                 M140 S0 ; Bed off\n\
                 M83\n\
                 G1 E-2 F2700 ; relieve nozzle pressure\n\
                 M82\n\
                 G1 Z{park_z} F900 ; lift clear, capped at the build height\n\
                 M84\n"
            }
            GcodeFlavor::RepRap => {
                "M104 S0\n\
                 M140 S0\n\
                 G91\n\
                 G1 E-2 F2700\n\
                 G1 Z10 F3000\n\
                 G90\n\
                 G1 X0 Y200 F3000\n\
                 M84\n"
            }
        }
    }

    /// Get layer change G-code.
    pub fn layer_change_gcode(&self) -> &'static str {
        match self {
            // `; CHANGE_LAYER` is the marker Bambu's firmware and tools key
            // on; `M73 L` (written by the generator) drives the layer count
            // on the printer's screen.
            GcodeFlavor::Bambu => "; CHANGE_LAYER\n",
            _ => "; layer change\n",
        }
    }

    /// Does this flavor support pressure advance / linear advance?
    pub fn supports_pressure_advance(&self) -> bool {
        matches!(self, GcodeFlavor::Klipper | GcodeFlavor::Marlin)
    }

    /// Does this flavor support input shaper?
    pub fn supports_input_shaper(&self) -> bool {
        matches!(self, GcodeFlavor::Klipper | GcodeFlavor::Bambu)
    }
}
