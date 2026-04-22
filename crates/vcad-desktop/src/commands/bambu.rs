//! Bambu Lab printer bridge.
//!
//! Wraps [`vcad_slicer_bambu::BambuPrinter`] behind a handful of Tauri
//! commands that mirror the HTTP API expected by
//! `packages/app/src/lib/print-relay.ts`. The frontend dispatcher routes
//! here whenever `isTauri()` is true; this is the only "printer" backend
//! the desktop shell ships.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;
use tauri::State;
use tokio::sync::Mutex;
use vcad_slicer_bambu::{discover_printers_async, BambuPrinter, PrintState};

/// Shared connection handle. A single printer at a time is plenty for
/// the current UI; if that changes, widen to a map keyed by serial.
pub struct BambuState(pub Arc<Mutex<Option<BambuPrinter>>>);

impl BambuState {
    /// Create an empty state.
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(None)))
    }
}

impl Default for BambuState {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize)]
pub struct RelayPrinterInfo {
    pub ip: String,
    pub serial: String,
    pub model: String,
    pub name: String,
}

#[derive(Serialize)]
pub struct RelayStatus {
    pub state: String,
    pub progress_percent: f64,
    pub layer_current: u32,
    pub layer_total: u32,
    pub time_remaining_min: u32,
    pub nozzle_temp: f64,
    pub nozzle_target: f64,
    pub bed_temp: f64,
    pub bed_target: f64,
    pub fan_speed: u8,
    pub filename: Option<String>,
}

fn state_label(state: &PrintState) -> String {
    match state {
        PrintState::Idle => "idle".into(),
        PrintState::Printing => "printing".into(),
        PrintState::Paused => "paused".into(),
        PrintState::Finished => "finished".into(),
        PrintState::Preparing => "preparing".into(),
        PrintState::Error(msg) => format!("error: {msg}"),
        PrintState::Unknown => "unknown".into(),
    }
}

#[tauri::command]
pub async fn bambu_discover() -> Result<Vec<RelayPrinterInfo>, String> {
    let found = discover_printers_async(Duration::from_secs(5))
        .await
        .map_err(|e| e.to_string())?;
    Ok(found
        .into_iter()
        .map(|p| RelayPrinterInfo {
            ip: p.ip.to_string(),
            serial: p.serial,
            model: p.model,
            name: p.name,
        })
        .collect())
}

#[tauri::command]
pub async fn bambu_connect(
    ip: String,
    serial: String,
    access_code: String,
    state: State<'_, BambuState>,
) -> Result<(), String> {
    let ip: IpAddr = ip
        .parse()
        .map_err(|e: std::net::AddrParseError| e.to_string())?;
    let printer = BambuPrinter::connect(ip, &serial, &access_code)
        .await
        .map_err(|e| e.to_string())?;
    *state.0.lock().await = Some(printer);
    Ok(())
}

#[tauri::command]
pub async fn bambu_status(state: State<'_, BambuState>) -> Result<RelayStatus, String> {
    let guard = state.0.lock().await;
    let printer = guard
        .as_ref()
        .ok_or_else(|| "no printer connected".to_string())?;
    let status = printer.status().await.map_err(|e| e.to_string())?;
    Ok(RelayStatus {
        state: state_label(&status.state),
        progress_percent: status.progress_percent,
        layer_current: status.layer_current,
        layer_total: status.layer_total,
        time_remaining_min: status.time_remaining_min,
        nozzle_temp: status.nozzle_temp,
        nozzle_target: status.nozzle_target,
        bed_temp: status.bed_temp,
        bed_target: status.bed_target,
        fan_speed: status.fan_speed,
        filename: status.filename,
    })
}

#[tauri::command]
pub async fn bambu_send_print(
    data_base64: String,
    filename: Option<String>,
    state: State<'_, BambuState>,
) -> Result<(), String> {
    let data = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| e.to_string())?;
    let filename = filename.unwrap_or_else(|| "vcad_print.3mf".to_string());
    let guard = state.0.lock().await;
    let printer = guard
        .as_ref()
        .ok_or_else(|| "no printer connected".to_string())?;
    printer
        .print_3mf(&filename, &data)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn bambu_control(action: String, state: State<'_, BambuState>) -> Result<(), String> {
    let guard = state.0.lock().await;
    let printer = guard
        .as_ref()
        .ok_or_else(|| "no printer connected".to_string())?;
    match action.as_str() {
        "pause" => printer.pause().await.map_err(|e| e.to_string()),
        "resume" => printer.resume().await.map_err(|e| e.to_string()),
        "stop" => printer.stop().await.map_err(|e| e.to_string()),
        other => Err(format!("unknown action: {other}")),
    }
}
