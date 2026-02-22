//! Print relay server for bridging the web app to local Bambu printers.
//!
//! The web app cannot directly access LAN printers (no MQTT/FTPS from browser).
//! This tiny HTTP server runs locally and proxies commands to the printer.
//!
//! Start with: `vcad print-server --port 7878`
//! The web app connects to `http://127.0.0.1:7878`.

use std::net::IpAddr;
use std::sync::Arc;

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

use vcad_slicer_bambu::{BambuPrinter, PrinterInfo, PrinterStatus};

/// Server state shared across handlers.
struct AppState {
    printer: Mutex<Option<BambuPrinter>>,
    last_status: Mutex<Option<PrinterStatus>>,
}

/// Start the print relay server.
pub async fn start_server(port: u16) -> anyhow::Result<()> {
    let state = Arc::new(AppState {
        printer: Mutex::new(None),
        last_status: Mutex::new(None),
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/printers", get(discover))
        .route("/connect", post(connect))
        .route("/status", get(status))
        .route("/print", post(print))
        .route("/control", post(control))
        .layer(CorsLayer::permissive())
        .with_state(state);

    let addr = format!("127.0.0.1:{}", port);
    println!("vcad print relay server listening on http://{}", addr);
    println!("Connect from the web app or use curl:");
    println!("  curl http://{}/health", addr);
    println!("  curl http://{}/printers", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

// ========== Handlers ==========

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
    }))
}

async fn discover() -> Result<Json<Vec<PrinterInfoResponse>>, (StatusCode, String)> {
    use std::time::Duration;

    let printers = vcad_slicer_bambu::discover_printers_async(Duration::from_secs(5))
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let response: Vec<PrinterInfoResponse> = printers
        .into_iter()
        .map(|p| PrinterInfoResponse {
            ip: p.ip.to_string(),
            serial: p.serial,
            model: p.model,
            name: p.name,
        })
        .collect();

    Ok(Json(response))
}

#[derive(Serialize)]
struct PrinterInfoResponse {
    ip: String,
    serial: String,
    model: String,
    name: String,
}

#[derive(Deserialize)]
struct ConnectRequest {
    ip: String,
    serial: String,
    access_code: String,
}

async fn connect(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ConnectRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let ip: IpAddr = req
        .ip
        .parse()
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid IP: {}", e)))?;

    let printer = BambuPrinter::connect(ip, &req.serial, &req.access_code)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    *state.printer.lock().await = Some(printer);

    Ok(Json(serde_json::json!({ "connected": true })))
}

async fn status(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let printer_lock = state.printer.lock().await;
    let printer = printer_lock
        .as_ref()
        .ok_or((StatusCode::BAD_REQUEST, "No printer connected".into()))?;

    let status = printer
        .status()
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    *state.last_status.lock().await = Some(status.clone());

    Ok(Json(serde_json::json!({
        "state": format!("{:?}", status.state),
        "progress_percent": status.progress_percent,
        "layer_current": status.layer_current,
        "layer_total": status.layer_total,
        "time_remaining_min": status.time_remaining_min,
        "nozzle_temp": status.nozzle_temp,
        "nozzle_target": status.nozzle_target,
        "bed_temp": status.bed_temp,
        "bed_target": status.bed_target,
        "fan_speed": status.fan_speed,
        "filename": status.filename,
    })))
}

#[derive(Deserialize)]
struct PrintRequest {
    /// Base64-encoded 3MF file data.
    data_base64: String,
    /// Filename for the print job.
    filename: Option<String>,
}

async fn print(
    State(state): State<Arc<AppState>>,
    Json(req): Json<PrintRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let printer_lock = state.printer.lock().await;
    let printer = printer_lock
        .as_ref()
        .ok_or((StatusCode::BAD_REQUEST, "No printer connected".into()))?;

    let data = base64::Engine::decode(
        &base64::engine::general_purpose::STANDARD,
        &req.data_base64,
    )
    .map_err(|e| (StatusCode::BAD_REQUEST, format!("Invalid base64: {}", e)))?;

    let filename = req.filename.unwrap_or_else(|| "vcad_print.3mf".to_string());

    printer
        .print_3mf(&filename, &data)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "status": "printing",
        "filename": filename,
    })))
}

#[derive(Deserialize)]
struct ControlRequest {
    action: String,
}

async fn control(
    State(state): State<Arc<AppState>>,
    Json(req): Json<ControlRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let printer_lock = state.printer.lock().await;
    let printer = printer_lock
        .as_ref()
        .ok_or((StatusCode::BAD_REQUEST, "No printer connected".into()))?;

    match req.action.as_str() {
        "pause" => printer.pause().await,
        "resume" => printer.resume().await,
        "stop" => printer.stop().await,
        _ => return Err((StatusCode::BAD_REQUEST, format!("Unknown action: {}", req.action))),
    }
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(serde_json::json!({
        "action": req.action,
        "status": "ok",
    })))
}
