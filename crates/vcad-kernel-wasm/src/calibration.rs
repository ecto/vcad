//! WASM bindings for the print-then-measure calibration engine.
//!
//! Thin JSON-over-the-boundary surface for `vcad-kernel-calibration` — the
//! TS module `packages/core/src/utils/print-calibration.ts` is a wrapper over
//! these three functions. Measurements arrive as ordered `[id, value]` pairs
//! (`Object.entries` on the JS side) so the `unknown` list preserves the
//! caller's insertion order exactly like the original TS did.

use wasm_bindgen::prelude::*;

use vcad_kernel_calibration::{
    build_calibration_report, default_tolerance, fingerprint_document, MeasurableKind,
    PrintPrediction, ReportOptions,
};

/// Content fingerprint (fnv1a-128 over the canonicalized JSON) of a document
/// IR or any JSON value, passed as a JSON string.
#[wasm_bindgen(js_name = calibrationFingerprintDocument)]
pub fn calibration_fingerprint_document(doc_json: &str) -> Result<String, JsError> {
    let doc: serde_json::Value = serde_json::from_str(doc_json)
        .map_err(|e| JsError::new(&format!("invalid document JSON: {e}")))?;
    Ok(fingerprint_document(&doc))
}

/// Default ± tolerance for a measurable kind ("dimension" | "diameter" |
/// "mass") that doesn't declare one.
#[wasm_bindgen(js_name = calibrationDefaultTolerance)]
pub fn calibration_default_tolerance(kind: &str, predicted: f64) -> Result<f64, JsError> {
    let kind: MeasurableKind = serde_json::from_value(serde_json::Value::String(kind.into()))
        .map_err(|_| JsError::new(&format!("unknown measurable kind: {kind}")))?;
    Ok(default_tolerance(kind, predicted))
}

/// Join a `PrintPrediction` (JSON) with measurements (JSON array of
/// `[id, value]` pairs) into a `CalibrationReport` (JSON). `options_json` is
/// the TS options object; the wrapper stamps `recorded_at` (this crate has
/// no clock).
#[wasm_bindgen(js_name = buildCalibrationReportJson)]
pub fn build_calibration_report_json(
    prediction_json: &str,
    measurements_json: &str,
    options_json: &str,
) -> Result<String, JsError> {
    let prediction: PrintPrediction = serde_json::from_str(prediction_json)
        .map_err(|e| JsError::new(&format!("invalid prediction JSON: {e}")))?;
    let measurements: Vec<(String, serde_json::Value)> = serde_json::from_str(measurements_json)
        .map_err(|e| JsError::new(&format!("invalid measurements JSON: {e}")))?;
    let options: ReportOptions = serde_json::from_str(options_json)
        .map_err(|e| JsError::new(&format!("invalid options JSON: {e}")))?;
    let report = build_calibration_report(&prediction, &measurements, &options);
    serde_json::to_string(&report).map_err(|e| JsError::new(&format!("serialize report: {e}")))
}
