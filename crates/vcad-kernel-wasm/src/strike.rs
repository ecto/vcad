//! WASM binding for the structural strike simulation
//! ([`vcad_kernel_acoustics::strike`]).
//!
//! Single JSON-over-the-boundary entry point, like `sheet_metal`: the MCP
//! server (and any other host) sends a [`strike::StrikeInput`] as JSON and
//! receives the full [`strike::StrikeResult`] back, with the optional WAV
//! bytes base64-encoded so the whole exchange stays one string.

use base64::Engine as _;
use serde::Serialize;
use vcad_kernel::vcad_kernel_acoustics::strike;
use wasm_bindgen::prelude::*;

/// Wire form of [`strike::StrikeResult`] with the WAV as base64.
#[derive(Serialize)]
struct StrikeResultWire {
    closed_form_hz: Vec<f64>,
    fem_hz: Vec<f64>,
    modes: Vec<strike::Mode>,
    spectrum_peaks: Vec<strike::SpectralPeak>,
    verdict: Option<strike::StrikeVerdict>,
    wav_base64: Option<String>,
}

/// Run the mallet-strike pipeline on a flat free-free bar.
///
/// `input_json` is a [`strike::StrikeInput`]; returns the result JSON with
/// `wav_base64` populated when `include_wav` was set.
#[wasm_bindgen(js_name = simulateStrikeKernel)]
pub fn simulate_strike_kernel(input_json: &str) -> Result<String, JsValue> {
    let input: strike::StrikeInput = serde_json::from_str(input_json)
        .map_err(|e| JsValue::from_str(&format!("bad strike input: {e}")))?;
    let result = strike::simulate_strike(&input);
    let wire = StrikeResultWire {
        closed_form_hz: result.closed_form_hz,
        fem_hz: result.fem_hz,
        modes: result.modes,
        spectrum_peaks: result.spectrum_peaks,
        verdict: result.verdict,
        wav_base64: result
            .wav
            .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
    };
    serde_json::to_string(&wire).map_err(|e| JsValue::from_str(&e.to_string()))
}

/// Parse a note name (`"C6"`, `"F#4"`, `"Bb3"`) to Hz. Errors on garbage.
#[wasm_bindgen(js_name = noteToHz)]
pub fn note_to_hz(note: &str) -> Result<f64, JsValue> {
    strike::note_to_hz(note).map_err(|e| JsValue::from_str(&e))
}
