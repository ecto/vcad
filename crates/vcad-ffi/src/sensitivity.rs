//! Parameter sensitivity for the native app's inspector: for every named
//! document parameter, `d(volume, mass, centre of mass)/dθ` per solid part,
//! through the differentiable seam (`vcad_eval::diff::document_parameter_gradient`)
//! — analytic seam derivatives, not finite differences.

use std::ffi::{c_char, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};

use serde_json::{json, Value};
use vcad_ir::Document;
use vcad_kernel_tessellate::TessellationParams;

/// Probe step used only to match surfaces between θ ± h (see
/// `document_parameter_gradient`); the derivatives themselves are analytic.
const PROBE_STEP: f64 = 1e-3;

fn tess() -> TessellationParams {
    TessellationParams {
        circle_segments: 64,
        height_segments: 2,
        ..Default::default()
    }
}

/// Sensitivities of every named parameter. `density_kg_m3` scales mass
/// (masses in grams). A parameter whose gradient can't be taken (it changes
/// topology at this value, drives no solid, …) carries an `error` instead.
pub fn document_sensitivity(doc: &Document, density_kg_m3: f64) -> Value {
    // kg/m³ → g/mm³.
    let density = density_kg_m3 * 1e-6;
    let env = vcad_ir::resolve_parameters(&doc.parameters).unwrap_or_default();
    let mut names: Vec<&String> = doc.parameters.keys().collect();
    names.sort();
    let parameters: Vec<Value> = names
        .into_iter()
        .map(|name| {
            let value = env.get(name.as_str()).copied();
            match vcad_eval::diff::document_parameter_gradient(
                doc,
                name,
                density,
                &tess(),
                PROBE_STEP,
            ) {
                Ok(parts) => json!({
                    "name": name,
                    "value": value,
                    "parts": parts.iter().map(|g| {
                        let p = &g.properties;
                        let d = &g.derivative;
                        json!({
                            "part_index": g.part_index,
                            "volume": p.volume,
                            "mass": p.mass,
                            "centroid": [p.centroid.x, p.centroid.y, p.centroid.z],
                            "d_volume": d.volume,
                            "d_mass": d.mass,
                            "d_centroid": [d.centroid.x, d.centroid.y, d.centroid.z],
                        })
                    }).collect::<Vec<_>>(),
                }),
                Err(e) => json!({ "name": name, "value": value, "error": e.to_string() }),
            }
        })
        .collect();
    json!({ "density_kg_m3": density_kg_m3, "parameters": parameters })
}

/// [`document_sensitivity`] over IR JSON (`json`/`json_len` UTF-8 bytes).
/// Returns owned JSON (free with `vcad_cam_free`), or null with
/// `vcad_last_error` set when the JSON isn't a document.
#[no_mangle]
pub extern "C" fn vcad_document_sensitivity(
    json: *const u8,
    json_len: usize,
    density_kg_m3: f64,
) -> *mut c_char {
    if json.is_null() {
        return std::ptr::null_mut();
    }
    crate::err::clear_error();
    catch_unwind(AssertUnwindSafe(|| {
        let bytes = unsafe { std::slice::from_raw_parts(json, json_len) };
        let Ok(text) = std::str::from_utf8(bytes) else {
            crate::err::set_error("document is not UTF-8");
            return std::ptr::null_mut();
        };
        let doc = match Document::from_json(text) {
            Ok(d) => d,
            Err(e) => {
                crate::err::set_error(format!("parse: {e}"));
                return std::ptr::null_mut();
            }
        };
        let out = document_sensitivity(&doc, density_kg_m3).to_string();
        CString::new(out).map_or(std::ptr::null_mut(), CString::into_raw)
    }))
    .unwrap_or_else(|_| {
        crate::err::set_error("vcad_document_sensitivity panicked");
        std::ptr::null_mut()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const BRACKET: &str =
        "[defparam t 9.0]\n[root [union [cube 70 40 t] [cube t 40 50]] \"aluminum\"]";

    fn rebuilt_volume(doc: &Document, t: f64) -> f64 {
        let mut d = doc.clone();
        d.parameters.get_mut("t").unwrap().value = vcad_ir::Expr::Number(t);
        let scene = vcad_eval::evaluate_document(&d, &vcad_eval::EvalOptions::default()).unwrap();
        scene.parts[0].solid.as_ref().unwrap().volume()
    }

    #[test]
    fn bracket_thickness_sensitivity_matches_a_rebuild() {
        let doc = vcad_loon::eval_vcad(BRACKET, None).unwrap();
        let out = document_sensitivity(&doc, 2700.0);
        let p = &out["parameters"][0];
        assert_eq!(p["name"], "t");
        assert_eq!(p["value"], 9.0);
        let part = &p["parts"][0];
        let dv = part["d_volume"].as_f64().unwrap();
        // V(t) = 70·40·t + t·40·(50 − t) − overlap… check against a rebuild FD.
        let h = 0.01;
        let fd = (rebuilt_volume(&doc, 9.0 + h) - rebuilt_volume(&doc, 9.0 - h)) / (2.0 * h);
        assert!((dv - fd).abs() / fd.abs() < 0.02, "seam {dv} vs fd {fd}");
        assert!(dv > 0.0, "thicker walls add volume");
        // Mass in grams at aluminium density: dm = dV · 2.7e-3.
        let dm = part["d_mass"].as_f64().unwrap();
        assert!(
            (dm - dv * 2.7e-3).abs() < 1e-6 * dv.abs().max(1.0),
            "{dm} vs {dv}"
        );
        assert_eq!(part["d_centroid"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn single_primitive_plate() {
        let doc = vcad_loon::eval_vcad("[defparam t 5.0]\n[root [cube 40 20 t] \"aluminum\"]", None).unwrap();
        let out = document_sensitivity(&doc, 2700.0);
        let part = &out["parameters"][0]["parts"][0];
        assert!(out["parameters"][0]["error"].is_null(), "{out}");
        assert!((part["d_volume"].as_f64().unwrap() - 800.0).abs() < 8.0, "{out}");
        assert!((part["d_centroid"][2].as_f64().unwrap() - 0.5).abs() < 0.02, "{out}");
    }

    #[test]
    fn documents_without_parameters_report_none() {
        let doc = vcad_loon::eval_vcad("[root [cube 10 10 10] \"steel\"]", None).unwrap();
        assert_eq!(document_sensitivity(&doc, 7850.0)["parameters"], json!([]));
    }
}
