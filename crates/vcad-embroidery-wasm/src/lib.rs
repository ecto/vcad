#![warn(missing_docs)]
//! WASM bindings for vcad embroidery.

use serde::Serialize;
use vcad_embroidery::{EmbPattern, StitchCommand, Thread};
use wasm_bindgen::prelude::*;

/// Initialize panic hook for better error messages.
#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// A WASM-friendly embroidery pattern wrapper.
#[wasm_bindgen]
pub struct WasmEmbPattern {
    inner: EmbPattern,
}

#[wasm_bindgen]
impl WasmEmbPattern {
    /// Get the total stitch count.
    #[wasm_bindgen(getter, js_name = stitchCount)]
    pub fn stitch_count(&self) -> usize {
        self.inner.total_stitch_count()
    }

    /// Get the number of thread colors.
    #[wasm_bindgen(getter, js_name = colorCount)]
    pub fn color_count(&self) -> usize {
        self.inner.threads.len()
    }

    /// Get the pattern width in mm.
    #[wasm_bindgen(getter)]
    pub fn width(&self) -> f64 {
        let stats = self.inner.stats();
        stats.width
    }

    /// Get the pattern height in mm.
    #[wasm_bindgen(getter)]
    pub fn height(&self) -> f64 {
        let stats = self.inner.stats();
        stats.height
    }

    /// Get pattern statistics as JSON.
    #[wasm_bindgen(js_name = statsJson)]
    pub fn stats_json(&self) -> Result<String, JsError> {
        let stats = self.inner.stats();
        serde_json::to_string(&stats).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Serialize the pattern to JSON.
    #[wasm_bindgen(js_name = toJson)]
    pub fn to_json(&self) -> Result<String, JsError> {
        serde_json::to_string(&self.inner).map_err(|e| JsError::new(&e.to_string()))
    }

    /// Deserialize a pattern from JSON.
    #[wasm_bindgen(js_name = fromJson)]
    pub fn from_json(json: &str) -> Result<WasmEmbPattern, JsError> {
        let inner: EmbPattern =
            serde_json::from_str(json).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(WasmEmbPattern { inner })
    }

    /// Get stitch paths for rendering.
    ///
    /// Returns a JSON-compatible value: array of `{threadIndex, color: [r,g,b], points: [[x,y], ...]}`.
    #[wasm_bindgen(js_name = getStitchPaths)]
    pub fn get_stitch_paths(&self) -> Result<JsValue, JsError> {
        let mut paths: Vec<StitchPathInfo> = Vec::new();

        for group in &self.inner.stitch_groups {
            let thread = self
                .inner
                .threads
                .get(group.thread_index)
                .cloned()
                .unwrap_or_else(|| Thread::new([0, 0, 0], "Unknown"));

            let mut points: Vec<[f64; 2]> = Vec::new();
            for cmd in &group.commands {
                match cmd {
                    StitchCommand::MoveTo { x, y } | StitchCommand::StitchTo { x, y } => {
                        points.push([*x, *y]);
                    }
                    StitchCommand::Jump { x, y } => {
                        if !points.is_empty() {
                            paths.push(StitchPathInfo {
                                thread_index: group.thread_index,
                                color: thread.color,
                                points: std::mem::take(&mut points),
                            });
                        }
                        points.push([*x, *y]);
                    }
                    StitchCommand::Trim | StitchCommand::End => {
                        if !points.is_empty() {
                            paths.push(StitchPathInfo {
                                thread_index: group.thread_index,
                                color: thread.color,
                                points: std::mem::take(&mut points),
                            });
                        }
                    }
                    _ => {}
                }
            }
            if !points.is_empty() {
                paths.push(StitchPathInfo {
                    thread_index: group.thread_index,
                    color: thread.color,
                    points,
                });
            }
        }

        serde_wasm_bindgen::to_value(&paths).map_err(|e| JsError::new(&e.to_string()))
    }
}

#[derive(Serialize)]
struct StitchPathInfo {
    #[serde(rename = "threadIndex")]
    thread_index: usize,
    color: [u8; 3],
    points: Vec<[f64; 2]>,
}

/// Read a PES file and return a WasmEmbPattern.
#[wasm_bindgen(js_name = readPes)]
pub fn read_pes(data: &[u8]) -> Result<WasmEmbPattern, JsError> {
    let inner =
        vcad_embroidery_pes::read_pes(data).map_err(|e| JsError::new(&e.to_string()))?;
    Ok(WasmEmbPattern { inner })
}

/// Write a WasmEmbPattern to PES format.
#[wasm_bindgen(js_name = writePes)]
pub fn write_pes(pattern: &WasmEmbPattern) -> Result<Vec<u8>, JsError> {
    vcad_embroidery_pes::write_pes(&pattern.inner).map_err(|e| JsError::new(&e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_embroidery::StitchGroup;

    #[test]
    fn test_pattern_roundtrip() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([255, 0, 0], "Red"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 5.0, y: 0.0 },
                StitchCommand::End,
            ],
        });

        let wasm_pattern = WasmEmbPattern { inner: pattern };
        let json = wasm_pattern.to_json().unwrap();
        let restored = WasmEmbPattern::from_json(&json).unwrap();

        assert_eq!(restored.stitch_count(), 1);
        assert_eq!(restored.color_count(), 1);
    }

    #[test]
    fn test_stats_json() {
        let mut pattern = EmbPattern::new();
        pattern.threads.push(Thread::new([0, 0, 0], "Black"));
        pattern.stitch_groups.push(StitchGroup {
            thread_index: 0,
            commands: vec![
                StitchCommand::MoveTo { x: 0.0, y: 0.0 },
                StitchCommand::StitchTo { x: 10.0, y: 0.0 },
                StitchCommand::End,
            ],
        });

        let wasm_pattern = WasmEmbPattern { inner: pattern };
        let stats = wasm_pattern.stats_json().unwrap();
        assert!(stats.contains("stitch_count"));
    }
}
