//! Typst WASM plugin for vcad.
//!
//! Exposes vcad's evaluator and drafting renderer to Typst documents via
//! the [wasm-minimal-protocol] bytes-in/bytes-out calling convention: every
//! export takes a JSON config plus source bytes and returns SVG (renders)
//! or JSON (data). The pure core lives in this module so it compiles and
//! tests natively; the `#[wasm_func]` shims are gated to `wasm32` in
//! [`wasm`].
//!
//! Typst plugins are sandboxed and must be pure — no filesystem, network,
//! or cross-call state — which these functions already are: string in,
//! string out, deterministic.
//!
//! [wasm-minimal-protocol]: https://github.com/typst-community/wasm-minimal-protocol

#![warn(missing_docs)]

use std::str::FromStr;

use serde::Deserialize;
use vcad_eval::{evaluate_document, EvalOptions, EvaluatedMesh};
use vcad_render::sheet::{render_sheet_svg_str, SheetOptions};
use vcad_render::{render_svg_str_opts, RenderAnnotations, SectionPlane, SvgOptions, View};

#[cfg(target_arch = "wasm32")]
mod wasm;

/// Render/inspect configuration, deserialized from the JSON dict the Typst
/// wrapper builds with `json.encode`. Every field is optional; defaults
/// match `vcad-render`'s CLI defaults.
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct Config {
    /// Source language: `"vcad"` (document JSON, default) or `"loon"`.
    pub format: Option<String>,
    /// Camera: `iso|front|side|top|hero|orbit:AZ,EL`.
    pub view: Option<String>,
    /// Pixels per millimeter (default 4).
    pub scale: Option<f64>,
    /// Omit the opaque paper background.
    pub transparent: Option<bool>,
    /// BRep-exact arcs for recognised curved edges (default true).
    pub exact_edges: Option<bool>,
    /// Cutaway plane, `x=N|y=N|z=N`.
    pub section: Option<String>,
    /// Draw the X/Y/Z origin gizmo.
    pub axes: Option<bool>,
    /// Label top-level parts.
    pub labels: Option<bool>,
    /// Draw overall bounding-box dimensions (mm).
    pub dims: Option<bool>,
    /// Frame the render on this part.
    pub focus: Option<String>,
    /// Sheet width in px for [`sheet`] renders (default 1600).
    pub sheet_width: Option<f64>,
    /// Title-block title for [`sheet`] renders.
    pub title: Option<String>,
}

impl Config {
    fn parse(config_json: &str) -> Result<Self, String> {
        if config_json.trim().is_empty() {
            return Ok(Self::default());
        }
        serde_json::from_str(config_json).map_err(|e| format!("bad config: {e}"))
    }

    /// Resolve the source to `.vcad` document JSON, evaluating loon first
    /// when `format` says so.
    fn to_vcad(&self, source: &str) -> Result<String, String> {
        match self.format.as_deref() {
            Some("loon") => {
                let doc = vcad_loon::eval_vcad(source, None)?;
                serde_json::to_string(&doc).map_err(|e| e.to_string())
            }
            None | Some("vcad") => Ok(source.to_string()),
            Some(other) => Err(format!("unknown format '{other}' (expected vcad|loon)")),
        }
    }

    fn svg_options(&self) -> Result<SvgOptions, String> {
        let view = match self.view.as_deref() {
            None => View::Isometric,
            Some(v) => View::from_str(v)?,
        };
        let section = match self.section.as_deref() {
            None => None,
            Some(s) => Some(SectionPlane::from_str(s)?),
        };
        Ok(SvgOptions {
            view,
            transparent: self.transparent.unwrap_or(false),
            exact_edges: self.exact_edges.unwrap_or(true),
            section,
            annotations: RenderAnnotations {
                axes: self.axes.unwrap_or(false),
                labels: self.labels.unwrap_or(false),
                dims: self.dims.unwrap_or(false),
            },
            focus: self.focus.clone(),
            ..Default::default()
        })
    }
}

/// Render source (`.vcad` JSON or loon, per config `format`) to a
/// self-contained drafting SVG.
pub fn render(config_json: &str, source: &str) -> Result<String, String> {
    let cfg = Config::parse(config_json)?;
    let raw = cfg.to_vcad(source)?;
    render_svg_str_opts(&raw, cfg.scale.unwrap_or(4.0), &cfg.svg_options()?)
}

/// Render source to a third-angle multi-view drawing sheet SVG
/// (front/side/top/iso plus title block).
pub fn sheet(config_json: &str, source: &str) -> Result<String, String> {
    let cfg = Config::parse(config_json)?;
    let raw = cfg.to_vcad(source)?;
    render_sheet_svg_str(
        &raw,
        &SheetOptions {
            width_px: cfg.sheet_width.unwrap_or(1600.0),
            title: cfg.title.clone().unwrap_or_else(|| "UNTITLED".to_string()),
        },
    )
}

/// Evaluate loon source to `.vcad` document JSON.
pub fn eval_loon(source: &str) -> Result<String, String> {
    let doc = vcad_loon::eval_vcad(source, None)?;
    serde_json::to_string(&doc).map_err(|e| e.to_string())
}

/// Measure the document: per-part and total volume (mm³), surface area
/// (mm²), bounding box, and center of mass, as JSON. This is what lets a
/// Typst document print numbers that are recomputed from the model on
/// every compile.
pub fn inspect(config_json: &str, source: &str) -> Result<String, String> {
    let cfg = Config::parse(config_json)?;
    let raw = cfg.to_vcad(source)?;
    let doc: vcad_ir::Document =
        serde_json::from_str(&raw).map_err(|e| format!("bad .vcad JSON: {e}"))?;
    let scene =
        evaluate_document(&doc, &EvalOptions::default()).map_err(|e| format!("eval: {e:?}"))?;
    if !scene.failures.is_empty() {
        let msgs: Vec<String> = scene
            .failures
            .iter()
            .map(|f| format!("{}: {}", f.scope, f.error))
            .collect();
        return Err(format!("evaluation failures: {}", msgs.join("; ")));
    }

    let mut parts = Vec::new();
    let mut total_volume = 0.0;
    let mut total_area = 0.0;
    let mut bbox_min = [f64::INFINITY; 3];
    let mut bbox_max = [f64::NEG_INFINITY; 3];
    let mut com_acc = [0.0; 3];
    for (i, part) in scene.parts.iter().enumerate() {
        let name = doc
            .roots
            .get(i)
            .and_then(|r| doc.nodes.get(&r.root))
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| format!("part-{i}"));
        let m = MeshProps::of(&part.mesh);
        total_volume += m.volume;
        total_area += m.area;
        for a in 0..3 {
            bbox_min[a] = bbox_min[a].min(m.bbox.0[a]);
            bbox_max[a] = bbox_max[a].max(m.bbox.1[a]);
            com_acc[a] += m.com[a] * m.volume;
        }
        parts.push(serde_json::json!({
            "name": name,
            "material": part.material,
            "volume": m.volume,
            "area": m.area,
            "bbox": { "min": m.bbox.0, "max": m.bbox.1 },
            "center-of-mass": m.com,
        }));
    }
    let com: Vec<f64> = com_acc
        .iter()
        .map(|c| {
            if total_volume > 0.0 {
                c / total_volume
            } else {
                0.0
            }
        })
        .collect();
    let size: Vec<f64> = (0..3)
        .map(|a| (bbox_max[a] - bbox_min[a]).max(0.0))
        .collect();
    serde_json::to_string(&serde_json::json!({
        "volume": total_volume,
        "area": total_area,
        "bbox": { "min": bbox_min, "max": bbox_max, "size": size },
        "center-of-mass": com,
        "parts": parts,
    }))
    .map_err(|e| e.to_string())
}

/// Plugin version string (the crate version).
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

/// Mesh-derived mass properties (divergence theorem over the triangle
/// soup). Uses the evaluated mesh rather than the BRep solid so it works
/// uniformly for every part, including mesh-backed ones.
struct MeshProps {
    volume: f64,
    area: f64,
    bbox: ([f64; 3], [f64; 3]),
    com: [f64; 3],
}

impl MeshProps {
    fn of(mesh: &EvaluatedMesh) -> Self {
        let p = &mesh.positions;
        let vert = |i: u32| {
            let i = i as usize * 3;
            [p[i] as f64, p[i + 1] as f64, p[i + 2] as f64]
        };
        let mut volume = 0.0;
        let mut area = 0.0;
        let mut com = [0.0; 3];
        for tri in mesh.indices.as_chunks::<3>().0 {
            let (a, b, c) = (vert(tri[0]), vert(tri[1]), vert(tri[2]));
            let cross = [
                (b[1] - a[1]) * (c[2] - a[2]) - (b[2] - a[2]) * (c[1] - a[1]),
                (b[2] - a[2]) * (c[0] - a[0]) - (b[0] - a[0]) * (c[2] - a[2]),
                (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0]),
            ];
            area += 0.5 * (cross[0].powi(2) + cross[1].powi(2) + cross[2].powi(2)).sqrt();
            // Signed tetra volume against the origin.
            let v = (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0;
            volume += v;
            for axis in 0..3 {
                com[axis] += v * (a[axis] + b[axis] + c[axis]) / 4.0;
            }
        }
        let volume_abs = volume.abs();
        let com = if volume_abs > 1e-12 {
            [com[0] / volume, com[1] / volume, com[2] / volume]
        } else {
            [0.0; 3]
        };
        let mut lo = [f64::INFINITY; 3];
        let mut hi = [f64::NEG_INFINITY; 3];
        for v in p.as_chunks::<3>().0 {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(v[axis] as f64);
                hi[axis] = hi[axis].max(v[axis] as f64);
            }
        }
        if p.is_empty() {
            lo = [0.0; 3];
            hi = [0.0; 3];
        }
        MeshProps {
            volume: volume_abs,
            area,
            bbox: (lo, hi),
            com,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUBE_LOON: &str = "[cube 40 20 10]";

    #[test]
    fn loon_render_svg() {
        let svg = render(r#"{"format":"loon","view":"iso"}"#, CUBE_LOON).unwrap();
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("</svg>"));
    }

    #[test]
    fn loon_sheet_svg() {
        let svg = sheet(r#"{"format":"loon","title":"CUBE"}"#, CUBE_LOON).unwrap();
        assert!(svg.contains("CUBE"));
    }

    #[test]
    fn inspect_cube_mass_props() {
        let out = inspect(r#"{"format":"loon"}"#, CUBE_LOON).unwrap();
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        let volume = v["volume"].as_f64().unwrap();
        assert!(
            (volume - 40.0 * 20.0 * 10.0).abs() < 1e-6,
            "volume {volume}"
        );
        let area = v["area"].as_f64().unwrap();
        assert!(
            (area - 2.0 * (800.0 + 400.0 + 200.0)).abs() < 1e-6,
            "area {area}"
        );
        let size = v["bbox"]["size"].as_array().unwrap();
        assert_eq!(size[2].as_f64().unwrap(), 10.0);
        let com = v["center-of-mass"].as_array().unwrap();
        assert!((com[0].as_f64().unwrap() - 20.0).abs() < 1e-6);
    }

    #[test]
    fn section_and_annotations() {
        let svg = render(
            r#"{"format":"loon","section":"z=5","dims":true,"axes":true}"#,
            CUBE_LOON,
        )
        .unwrap();
        assert!(svg.starts_with("<svg"));
    }

    #[test]
    fn bad_view_is_error() {
        assert!(render(r#"{"format":"loon","view":"wat"}"#, CUBE_LOON).is_err());
    }

    #[test]
    fn eval_loon_roundtrips() {
        let json = eval_loon(CUBE_LOON).unwrap();
        let doc: vcad_ir::Document = serde_json::from_str(&json).unwrap();
        assert_eq!(doc.roots.len(), 1);
    }
}
