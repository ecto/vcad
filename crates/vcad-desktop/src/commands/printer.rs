use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct PrinterInfo {
    pub id: String,
    pub name: String,
    pub printer_type: String,
    pub connected: bool,
}

#[tauri::command]
pub fn discover_printers() -> Vec<PrinterInfo> {
    // TODO: implement USB/network printer discovery
    Vec::new()
}

#[tauri::command]
pub fn send_to_printer(printer_id: String, gcode: String) -> Result<(), String> {
    // TODO: implement gcode streaming to printer
    Err(format!(
        "printer support not yet implemented (printer_id={}, gcode_len={})",
        printer_id,
        gcode.len()
    ))
}
