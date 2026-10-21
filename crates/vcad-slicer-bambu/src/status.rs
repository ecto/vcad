//! Printer status types.

use serde::{Deserialize, Serialize};

/// Printer state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum PrintState {
    /// Printer is idle.
    Idle,
    /// Print in progress.
    Printing,
    /// Print paused.
    Paused,
    /// Print finished.
    Finished,
    /// Error state.
    Error(String),
    /// Preparing to print.
    Preparing,
    /// Unknown state.
    #[default]
    Unknown,
}

impl PrintState {
    /// Parse from Bambu status string.
    pub fn from_bambu_status(status: &str) -> Self {
        match status.to_lowercase().as_str() {
            "idle" | "standby" => Self::Idle,
            "printing" | "running" => Self::Printing,
            "paused" | "pause" => Self::Paused,
            "finished" | "finish" | "completed" => Self::Finished,
            "preparing" | "prepare" => Self::Preparing,
            s if s.contains("error") || s.contains("fail") => Self::Error(status.to_string()),
            _ => Self::Unknown,
        }
    }
}

/// AMS (Automatic Material System) status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AmsStatus {
    /// AMS unit statuses.
    pub units: Vec<AmsUnit>,
    /// Currently active AMS unit.
    pub active_unit: Option<usize>,
    /// Currently active slot.
    pub active_slot: Option<usize>,
}

/// Single AMS unit status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmsUnit {
    /// Unit ID.
    pub id: u8,
    /// Slots in this unit.
    pub slots: Vec<AmsSlot>,
    /// Humidity level (0-100).
    pub humidity: Option<u8>,
    /// Temperature (°C).
    pub temperature: Option<f32>,
}

/// AMS slot status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AmsSlot {
    /// Slot ID (0-3).
    pub id: u8,
    /// Filament type (e.g., "PLA", "PETG").
    pub filament_type: Option<String>,
    /// Filament color (hex).
    pub color: Option<String>,
    /// Remaining filament percentage.
    pub remaining: Option<u8>,
}

/// Full printer status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PrinterStatus {
    /// Current print state.
    pub state: PrintState,
    /// Print progress (0-100).
    pub progress_percent: f64,
    /// Current layer number.
    pub layer_current: u32,
    /// Total layer count.
    pub layer_total: u32,
    /// Remaining time in minutes.
    pub time_remaining_min: u32,
    /// Elapsed time in minutes.
    pub time_elapsed_min: u32,
    /// Nozzle temperature (°C).
    pub nozzle_temp: f64,
    /// Target nozzle temperature.
    pub nozzle_target: f64,
    /// Bed temperature (°C).
    pub bed_temp: f64,
    /// Target bed temperature.
    pub bed_target: f64,
    /// Chamber temperature (if available).
    pub chamber_temp: Option<f64>,
    /// Fan speed (0-100).
    pub fan_speed: u8,
    /// Current print speed.
    pub print_speed: u8,
    /// AMS status (if available).
    pub ams_status: Option<AmsStatus>,
    /// Current file name.
    pub filename: Option<String>,
    /// WiFi signal strength (dBm).
    pub wifi_signal: Option<i32>,
    /// Firmware version.
    pub firmware_version: Option<String>,
    /// Filament type feeding the nozzle (`PLA`, `PETG`, …): the active AMS
    /// tray, or the external spool when no AMS tray is active. `None` when
    /// the printer doesn't say (the print gate then refuses).
    #[serde(default)]
    pub loaded_filament: Option<String>,
}

/// Resolve the active filament from a `print` report object.
///
/// `ams.tray_now` is the global tray index (`ams_id * 4 + slot`); `254`
/// means the external spool (`vt_tray`), `255` means nothing loaded.
fn loaded_filament(print: &serde_json::Value) -> Option<String> {
    let tray_type = |v: &serde_json::Value| {
        v.get("tray_type")
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
    };
    let tray_now = print
        .get("ams")
        .and_then(|a| a.get("tray_now"))
        .and_then(|t| {
            t.as_str()
                .map(str::to_string)
                .or(t.as_u64().map(|n| n.to_string()))
        });
    match tray_now.as_deref() {
        Some("255") => None,
        None | Some("254") => print.get("vt_tray").and_then(tray_type),
        Some(n) => {
            let n: usize = n.parse().ok()?;
            let unit = print.get("ams")?.get("ams")?.get(n / 4)?;
            tray_type(unit.get("tray")?.get(n % 4)?)
        }
    }
}

impl PrinterStatus {
    /// Parse from Bambu MQTT message.
    pub fn from_mqtt_payload(payload: &serde_json::Value) -> Self {
        let mut status = Self::default();

        if let Some(print) = payload.get("print") {
            // State
            if let Some(state_str) = print.get("gcode_state").and_then(|v| v.as_str()) {
                status.state = PrintState::from_bambu_status(state_str);
            }

            // Progress
            if let Some(pct) = print.get("mc_percent").and_then(|v| v.as_f64()) {
                status.progress_percent = pct;
            }

            // Layers
            if let Some(layer) = print.get("layer_num").and_then(|v| v.as_u64()) {
                status.layer_current = layer as u32;
            }
            if let Some(total) = print.get("total_layer_num").and_then(|v| v.as_u64()) {
                status.layer_total = total as u32;
            }

            // Time
            if let Some(remaining) = print.get("mc_remaining_time").and_then(|v| v.as_u64()) {
                status.time_remaining_min = remaining as u32;
            }

            // Temperatures
            if let Some(temp) = print.get("nozzle_temper").and_then(|v| v.as_f64()) {
                status.nozzle_temp = temp;
            }
            if let Some(target) = print.get("nozzle_target_temper").and_then(|v| v.as_f64()) {
                status.nozzle_target = target;
            }
            if let Some(temp) = print.get("bed_temper").and_then(|v| v.as_f64()) {
                status.bed_temp = temp;
            }
            if let Some(target) = print.get("bed_target_temper").and_then(|v| v.as_f64()) {
                status.bed_target = target;
            }

            // Fan
            if let Some(fan) = print.get("cooling_fan_speed").and_then(|v| v.as_u64()) {
                status.fan_speed = (fan * 100 / 15) as u8; // Bambu uses 0-15
            }

            // Speed
            if let Some(speed) = print.get("spd_lvl").and_then(|v| v.as_u64()) {
                status.print_speed = speed as u8;
            }

            // Filename
            if let Some(name) = print.get("gcode_file").and_then(|v| v.as_str()) {
                status.filename = Some(name.to_string());
            }

            // WiFi
            if let Some(signal) = print.get("wifi_signal").and_then(|v| v.as_str()) {
                status.wifi_signal = signal.replace("dBm", "").trim().parse().ok();
            }

            status.loaded_filament = loaded_filament(print);
        }

        if let Some(ver) = payload
            .get("info")
            .and_then(|i| i.get("module"))
            .and_then(|m| m.as_array())
            .and_then(|mods| {
                mods.iter()
                    .find(|m| m.get("name").and_then(|n| n.as_str()) == Some("ota"))
            })
            .and_then(|m| m.get("sw_ver"))
            .and_then(|v| v.as_str())
        {
            status.firmware_version = Some(ver.to_string());
        }

        status
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn loaded_filament_follows_tray_now() {
        let ams = json!({
            "tray_now": "5",
            "ams": [
                {"tray": [{"tray_type": "PLA"}, {}, {}, {}]},
                {"tray": [{}, {"tray_type": "PETG"}, {}, {}]}
            ]
        });
        let p = json!({"print": {"gcode_state": "IDLE", "ams": ams,
                                 "vt_tray": {"tray_type": "TPU"}}});
        let s = PrinterStatus::from_mqtt_payload(&p);
        assert_eq!(s.state, PrintState::Idle);
        assert_eq!(s.loaded_filament.as_deref(), Some("PETG"));

        let ext = json!({"print": {"ams": {"tray_now": "254"}, "vt_tray": {"tray_type": "PLA"}}});
        assert_eq!(
            PrinterStatus::from_mqtt_payload(&ext)
                .loaded_filament
                .as_deref(),
            Some("PLA")
        );
        // No AMS on the P1S: external spool.
        let no_ams = json!({"print": {"vt_tray": {"tray_type": "PLA"}}});
        assert_eq!(
            PrinterStatus::from_mqtt_payload(&no_ams)
                .loaded_filament
                .as_deref(),
            Some("PLA")
        );
        let none = json!({"print": {"ams": {"tray_now": "255"}, "vt_tray": {"tray_type": "PLA"}}});
        assert_eq!(
            PrinterStatus::from_mqtt_payload(&none).loaded_filament,
            None
        );
        let blank = json!({"print": {"vt_tray": {"tray_type": ""}}});
        assert_eq!(
            PrinterStatus::from_mqtt_payload(&blank).loaded_filament,
            None
        );
    }

    #[test]
    fn firmware_from_info_module() {
        let p = json!({"info": {"module": [
            {"name": "mc", "sw_ver": "00.00.01"},
            {"name": "ota", "sw_ver": "01.09.01.00"}
        ]}});
        assert_eq!(
            PrinterStatus::from_mqtt_payload(&p)
                .firmware_version
                .as_deref(),
            Some("01.09.01.00")
        );
    }
}
