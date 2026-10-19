//! Per-part evaluation accounting: what every authored part became.
//!
//! The renderer used to drop a part that failed to evaluate (or whose part
//! definition was missing) without a word — a 122-part assembly rendered as
//! "evaluated 108 part(s)" and nothing said which 14 were missing. Every
//! evaluation now keeps a [`PartLog`] per authored part, the render paths
//! print the dropped ones to stderr ([`warn_dropped`]), and [`eval_report`]
//! turns the log into the machine-readable report behind
//! `vcad-render --eval-json`: evaluate, never render.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;

use serde::Serialize;

/// Where a part came from in the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PartKind {
    /// A visible scene root (`[root ...]`).
    Root,
    /// An assembly instance (`[instance ...]` of a `[part ...]`).
    Instance,
}

/// One authored part's evaluation outcome, before any geometry stats.
#[derive(Debug, Clone)]
pub struct PartLog {
    pub(crate) kind: PartKind,
    /// Root node id (scene roots) or instance id.
    pub(crate) id: String,
    pub(crate) name: String,
    /// `None` when the part produced geometry; otherwise why it did not.
    pub(crate) failure: Option<String>,
    /// Wall time of this part's evaluation, when timed. A part definition
    /// shared by several instances is charged to the first one only.
    pub(crate) eval_ms: Option<f64>,
}

impl PartLog {
    pub(crate) fn ok(kind: PartKind, id: String, name: String, eval_ms: Option<f64>) -> Self {
        PartLog {
            kind,
            id,
            name,
            failure: None,
            eval_ms,
        }
    }

    pub(crate) fn failed(kind: PartKind, id: String, name: String, why: String) -> Self {
        PartLog {
            kind,
            id,
            name,
            failure: Some(why),
            eval_ms: None,
        }
    }
}

/// Monotonic stamp, or `None` when untimed (and always on wasm32, where
/// `Instant::now` panics).
pub(crate) fn now(timed: bool) -> Option<std::time::Instant> {
    #[cfg(not(target_arch = "wasm32"))]
    if timed {
        return Some(std::time::Instant::now());
    }
    let _ = timed;
    None
}

pub(crate) fn elapsed_ms(t0: Option<std::time::Instant>) -> Option<f64> {
    t0.map(|t| t.elapsed().as_secs_f64() * 1e3)
}

/// Per-root wall times, recovered from the evaluator's `on_root` callback:
/// roots evaluate in order, so the gap between consecutive landings is what
/// each one cost.
pub(crate) struct Stamps {
    start: Option<std::time::Instant>,
    landed: Rc<RefCell<Vec<std::time::Instant>>>,
}

impl Stamps {
    pub(crate) fn new(timed: bool) -> Self {
        Stamps {
            start: now(timed),
            landed: Rc::default(),
        }
    }

    pub(crate) fn on_root(&self) -> Option<vcad_eval::OnRoot> {
        self.start?;
        let landed = Rc::clone(&self.landed);
        Some(Box::new(move |_, _, _| {
            landed.borrow_mut().push(std::time::Instant::now());
        }))
    }

    pub(crate) fn deltas_ms(&self) -> Vec<f64> {
        let Some(mut prev) = self.start else {
            return Vec::new();
        };
        self.landed
            .borrow()
            .iter()
            .map(|&t| {
                let d = t.duration_since(prev).as_secs_f64() * 1e3;
                prev = t;
                d
            })
            .collect()
    }
}

/// Log entries for the document's visible scene roots, index-aligned with
/// `scene.parts`. Part-definition prototypes (drawn through their instances
/// instead) are not parts of their own and get no entry.
pub(crate) fn root_log(
    doc: &vcad_ir::Document,
    visible_roots: &[&vcad_ir::SceneEntry],
    proto_roots: &HashSet<vcad_ir::NodeId>,
    scene: &vcad_eval::EvaluatedScene,
    root_ms: &[f64],
) -> Vec<PartLog> {
    // Failures are keyed by the root's index among *all* roots.
    let all_index: HashMap<vcad_ir::NodeId, usize> = doc
        .roots
        .iter()
        .enumerate()
        .map(|(i, r)| (r.root, i))
        .collect();
    let mut log = Vec::new();
    for (i, part) in scene.parts.iter().enumerate() {
        let Some(entry) = visible_roots.get(i) else {
            break;
        };
        if proto_roots.contains(&entry.root) {
            continue;
        }
        let id = entry.root.to_string();
        let name = doc
            .nodes
            .get(&entry.root)
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| format!("root {id}"));
        // Same test the render path applies: a B-rep, or a non-empty mesh.
        if part.solid.is_some() || !part.mesh.indices.is_empty() {
            log.push(PartLog::ok(
                PartKind::Root,
                id,
                name,
                root_ms.get(i).copied(),
            ));
            continue;
        }
        let scope = all_index.get(&entry.root).map(|j| format!("root[{j}]"));
        let why = scene
            .failures
            .iter()
            .find(|f| Some(&f.scope) == scope.as_ref() || f.node_id == entry.root)
            .map(|f| f.error.clone())
            .unwrap_or_else(|| "produced no geometry".to_string());
        let mut entry = PartLog::failed(PartKind::Root, id, name, why);
        entry.eval_ms = root_ms.get(i).copied();
        log.push(entry);
    }
    log
}

/// The one-line stderr summary of dropped parts, plus one line per part.
/// Silent when nothing was dropped.
pub(crate) fn warn_dropped(log: &[PartLog]) {
    let dropped: Vec<&PartLog> = log.iter().filter(|p| p.failure.is_some()).collect();
    if dropped.is_empty() {
        return;
    }
    eprintln!(
        "vcad-render: {} of {} part(s) produced no geometry and are omitted:",
        dropped.len(),
        log.len()
    );
    for p in dropped {
        eprintln!(
            "  - {} ({}): {}",
            p.name,
            p.id,
            p.failure.as_deref().unwrap_or_default()
        );
    }
}

/// One part's line in [`EvalReport`].
#[derive(Debug, Clone, Serialize)]
pub struct PartReport {
    /// Authored name (node name / instance name), else the id.
    pub name: String,
    /// Root node id or instance id.
    pub id: String,
    /// Scene root or assembly instance.
    pub kind: PartKind,
    /// True when the part produced geometry that a render would draw.
    pub ok: bool,
    /// Why the part produced nothing (only when `ok` is false).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// World-space axis-aligned bounds `[[min], [max]]` in mm, as posed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bbox: Option<[[f64; 3]; 2]>,
    /// Enclosed volume in mm³ (signed-tetrahedron sum of the tessellation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f64>,
    /// Triangle count of the tessellation the renderer would draw.
    pub triangles: usize,
    /// Wall-clock evaluation time. For instances sharing one part
    /// definition, only the first instance carries the cost.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eval_ms: Option<f64>,
}

/// Machine-readable outcome of evaluating a document without rendering it.
#[derive(Debug, Clone, Serialize)]
pub struct EvalReport {
    /// Authored parts (visible roots + assembly instances).
    pub parts_total: usize,
    /// Parts that produced geometry.
    pub parts_ok: usize,
    /// Parts that produced nothing (see each part's `reason`).
    pub parts_failed: usize,
    /// Whole-document evaluation wall time.
    pub eval_ms: f64,
    /// One entry per authored part, in document order (roots, then instances).
    pub parts: Vec<PartReport>,
}

/// Evaluate a `.vcad` document and report on every part — name, ok/failed
/// with reason, posed bbox, volume, triangle count, eval time — without
/// rendering anything.
#[cfg(not(target_arch = "wasm32"))]
pub fn eval_report(raw_vcad: &str) -> Result<EvalReport, String> {
    let t0 = std::time::Instant::now();
    let ev = crate::evaluate_vcad_document_inner(raw_vcad, true)?;
    let eval_ms = t0.elapsed().as_secs_f64() * 1e3;

    let world = if ev.parts.is_empty() {
        HashMap::new()
    } else {
        crate::assembly_world_transforms(&ev.doc)?
    };
    let fallbacks: HashMap<&str, Option<vcad_ir::Transform3D>> = ev
        .doc
        .instances
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|i| (i.id.as_str(), i.transform))
        .collect();
    let mut statics = ev.statics.into_iter();
    let mut parts = ev.parts.into_iter();

    let mut out = Vec::with_capacity(ev.log.len());
    for entry in ev.log {
        let solid = match (&entry.failure, entry.kind) {
            (Some(_), _) => None,
            (None, PartKind::Root) => statics.next().map(|s| s.solid),
            (None, PartKind::Instance) => parts.next().map(|p| {
                let fallback = fallbacks
                    .get(p.instance_id.as_str())
                    .copied()
                    .unwrap_or(None);
                crate::place_part(p, &world, fallback).solid
            }),
        };
        let stats = solid.as_ref().map(mesh_stats);
        out.push(PartReport {
            name: entry.name,
            id: entry.id,
            kind: entry.kind,
            ok: entry.failure.is_none(),
            reason: entry.failure,
            bbox: stats.as_ref().and_then(|s| s.bbox),
            volume: stats.as_ref().map(|s| s.volume),
            triangles: stats.as_ref().map_or(0, |s| s.triangles),
            eval_ms: entry.eval_ms,
        });
    }
    let parts_ok = out.iter().filter(|p| p.ok).count();
    Ok(EvalReport {
        parts_total: out.len(),
        parts_ok,
        parts_failed: out.len() - parts_ok,
        eval_ms,
        parts: out,
    })
}

#[cfg(not(target_arch = "wasm32"))]
struct MeshStats {
    bbox: Option<[[f64; 3]; 2]>,
    volume: f64,
    triangles: usize,
}

#[cfg(not(target_arch = "wasm32"))]
fn mesh_stats(solid: &vcad_kernel::Solid) -> MeshStats {
    let mesh = solid.to_mesh(crate::tessellation_segments(None));
    let v = |i: u32| {
        let i = i as usize * 3;
        [
            mesh.vertices[i] as f64,
            mesh.vertices[i + 1] as f64,
            mesh.vertices[i + 2] as f64,
        ]
    };
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in mesh.vertices.chunks_exact(3) {
        for k in 0..3 {
            lo[k] = lo[k].min(p[k] as f64);
            hi[k] = hi[k].max(p[k] as f64);
        }
    }
    let mut six_v = 0.0;
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
        six_v += a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
            + a[2] * (b[0] * c[1] - b[1] * c[0]);
    }
    MeshStats {
        bbox: lo[0].is_finite().then_some([lo, hi]),
        volume: six_v / 6.0,
        triangles: mesh.indices.len() / 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube moved to x = 10..11 and then `[rotate 90 90 0]`.
    /// R = Rx·Ry·Rz turns about Z first (a no-op here), then Y — carrying
    /// +X to −Z — then X, carrying −Z to +Y. So the cube must land at
    /// y = 10..11. The other order (X first) would leave it at z = −11..−10.
    /// This pins the order the docs state for `[rotate rx ry rz]`.
    const ROTATED_CUBE_PLUS_GHOSTS: &str = r#"{
      "version": "0.1",
      "nodes": {
        "1": { "id": 1, "name": "cube", "op": { "type": "Cube", "size": { "x": 1, "y": 1, "z": 1 } } },
        "2": { "id": 2, "op": { "type": "Translate", "child": 1, "offset": { "x": 10, "y": 0, "z": 0 } } },
        "3": { "id": 3, "name": "rotated", "op": { "type": "Rotate", "child": 2, "angles": { "x": 90, "y": 90, "z": 0 } } }
      },
      "materials": {},
      "part_materials": {},
      "roots": [{ "root": 3, "material": "default" }],
      "partDefs": { "p": { "id": "p", "name": "p", "root": 1 } },
      "instances": [
        { "id": "good", "partDefId": "p" },
        { "id": "ghost", "partDefId": "missing" }
      ]
    }"#;

    #[test]
    fn report_lists_every_part_and_pins_rotation_order() {
        let r = eval_report(ROTATED_CUBE_PLUS_GHOSTS).expect("report");
        assert_eq!(r.parts_total, 3, "{r:#?}");
        assert_eq!(r.parts_ok, 2);
        assert_eq!(r.parts_failed, 1);

        let rotated = &r.parts[0];
        assert_eq!(rotated.name, "rotated");
        assert_eq!(rotated.kind, PartKind::Root);
        assert!(rotated.ok);
        assert_eq!(rotated.triangles, 12);
        assert!((rotated.volume.unwrap() - 1.0).abs() < 1e-6);
        assert!(rotated.eval_ms.is_some());
        let [lo, hi] = rotated.bbox.unwrap();
        let near = |a: f64, b: f64| (a - b).abs() < 1e-4;
        assert!(
            near(lo[1], 10.0) && near(hi[1], 11.0),
            "bbox {lo:?}..{hi:?}"
        );

        let ghost = r
            .parts
            .iter()
            .find(|p| p.id == "ghost")
            .expect("ghost listed");
        assert!(!ghost.ok);
        assert!(
            ghost.reason.as_deref().unwrap().contains("missing"),
            "{:?}",
            ghost.reason
        );
        assert_eq!(ghost.triangles, 0);
    }
}
