//! Bambu printer commands.

use serde_json::json;

/// Command to send to the printer.
#[derive(Debug, Clone)]
pub enum PrinterCommand {
    /// Request printer status push.
    PushAll,
    /// Start a print job.
    PrintStart {
        /// File path on SD card or URL.
        file: String,
        /// Plate index (0 for default).
        plate_index: u32,
        /// Use AMS.
        use_ams: bool,
        /// AMS mapping (filament slot per object).
        ams_mapping: Vec<u32>,
        /// Bed leveling before print.
        bed_leveling: bool,
        /// Flow calibration.
        flow_calibration: bool,
        /// Vibration calibration.
        vibration_calibration: bool,
        /// Skip objects (by index).
        skip_objects: Vec<u32>,
    },
    /// Pause current print.
    PrintPause,
    /// Resume paused print.
    PrintResume,
    /// Stop current print.
    PrintStop,
    /// Set print speed level (1-4, where 4 is ludicrous).
    SetSpeed(u8),
    /// Set nozzle temperature.
    SetNozzleTemp(u32),
    /// Set bed temperature.
    SetBedTemp(u32),
    /// Control chamber LED.
    SetLed {
        /// LED node ("chamber_light" or "work_light").
        node: String,
        /// LED mode ("on", "off", "flashing").
        mode: String,
    },
    /// Send G-code line.
    GcodeLine(String),
    /// Unload filament from AMS.
    AmsUnload,
    /// Control AMS filament.
    AmsControl {
        /// Target AMS unit.
        ams_id: u8,
        /// Target slot.
        slot_id: u8,
    },
}

impl PrinterCommand {
    /// Convert command to JSON payload.
    pub fn to_json(&self) -> serde_json::Value {
        match self {
            PrinterCommand::PushAll => json!({
                "pushing": {
                    "sequence_id": "0",
                    "command": "pushall"
                }
            }),

            PrinterCommand::PrintStart {
                file,
                plate_index: _,
                use_ams,
                ams_mapping,
                bed_leveling,
                flow_calibration,
                vibration_calibration,
                skip_objects,
            } => {
                let mut cmd = json!({
                    "print": {
                        "sequence_id": "0",
                        "command": "project_file",
                        "param": file,
                        "subtask_name": file,
                        "url": format!("ftp://{}", file),
                        "bed_type": "auto",
                        "timelapse": false,
                        "bed_leveling": bed_leveling,
                        "flow_cali": flow_calibration,
                        "vibration_cali": vibration_calibration,
                        "layer_inspect": false,
                        "use_ams": use_ams,
                        "profile_id": "0",
                        "project_id": "0",
                        "subtask_id": "0",
                        "task_id": "0"
                    }
                });

                if let Some(print) = cmd.get_mut("print") {
                    if !ams_mapping.is_empty() {
                        print["ams_mapping"] = json!(ams_mapping);
                    }
                    if !skip_objects.is_empty() {
                        print["skip_objects"] = json!(skip_objects);
                    }
                }

                cmd
            }

            PrinterCommand::PrintPause => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "pause"
                }
            }),

            PrinterCommand::PrintResume => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "resume"
                }
            }),

            PrinterCommand::PrintStop => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "stop"
                }
            }),

            PrinterCommand::SetSpeed(level) => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "print_speed",
                    "param": level.to_string()
                }
            }),

            PrinterCommand::SetNozzleTemp(temp) => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "gcode_line",
                    "param": format!("M104 S{}", temp)
                }
            }),

            PrinterCommand::SetBedTemp(temp) => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "gcode_line",
                    "param": format!("M140 S{}", temp)
                }
            }),

            PrinterCommand::SetLed { node, mode } => json!({
                "system": {
                    "sequence_id": "0",
                    "command": "ledctrl",
                    "led_node": node,
                    "led_mode": mode,
                    "led_on_time": 500,
                    "led_off_time": 500,
                    "loop_times": 0,
                    "interval_time": 0
                }
            }),

            PrinterCommand::GcodeLine(gcode) => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "gcode_line",
                    "param": gcode
                }
            }),

            PrinterCommand::AmsUnload => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "ams_change_filament",
                    "target": 255,
                    "curr_temp": 0,
                    "tar_temp": 0
                }
            }),

            PrinterCommand::AmsControl { ams_id, slot_id } => json!({
                "print": {
                    "sequence_id": "0",
                    "command": "ams_change_filament",
                    "target": (ams_id * 4 + slot_id),
                    "curr_temp": 220,
                    "tar_temp": 220
                }
            }),
        }
    }

    /// Get the MQTT topic suffix for this command.
    pub fn topic_suffix(&self) -> &'static str {
        match self {
            PrinterCommand::PushAll => "request",
            _ => "request",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_push_all_command() {
        let cmd = PrinterCommand::PushAll;
        let json = cmd.to_json();
        assert!(json.get("pushing").is_some());
    }

    #[test]
    fn test_pause_command() {
        let cmd = PrinterCommand::PrintPause;
        let json = cmd.to_json();
        assert_eq!(json["print"]["command"].as_str(), Some("pause"));
    }
}
