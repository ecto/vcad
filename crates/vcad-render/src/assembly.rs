//! `--assembly` / `--explode`: render several posed parts as one scene.
//!
//! # Why this is JSON and not the assembly document
//!
//! The real home for this is #844's assembly model, where a posed instance is
//! `[PosedInstanceEntry name part-name x y z rx ry rz ex ey ez]` — a named
//! part reference, a full pose, and an exploded-view offset carried as data on
//! the assembly rather than hand-coded in each viewer. That work is on its own
//! branch and not yet on main, so this reads a small JSON with **exactly those
//! eleven fields under exactly those names**:
//!
//! ```json
//! {
//!   "parts":     [ { "name": "shell", "source": "shell.loon" } ],
//!   "instances": [ { "name": "shell-1", "part": "shell",
//!                    "x": 0, "y": 0, "z": 0,
//!                    "rx": 0, "ry": 0, "rz": 0,
//!                    "ex": 0, "ey": 0, "ez": 1 } ]
//! }
//! ```
//!
//! Converging is then mechanical: when the assembly document lands, this
//! module keeps its transform and explode maths and swaps its input for
//! `Document::instances`. The field names and the `Rz·Ry·Rx` Euler
//! convention are chosen to match, so no numbers have to be reinterpreted.
//!
//! # Exploding
//!
//! `--explode f` adds `f * (ex, ey, ez)` to each instance's translation. The
//! offset direction is the assembly author's — it says which way a part comes
//! off — and the factor is the viewer's. `f = 0` is the assembled view, so a
//! build sheet can render the same file assembled and exploded without
//! maintaining two transform tables. Every rana viewer and build sheet
//! currently hand-codes both.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use vcad_kernel::Solid;

use crate::PosedPart;

/// A parts-and-poses assembly.
#[derive(Debug, Deserialize)]
pub struct AssemblySpec {
    /// Part definitions, each naming a `.loon` or `.vcad` source.
    pub parts: Vec<PartDef>,
    /// Posed instances of those parts.
    pub instances: Vec<Instance>,
}

/// A named part and where its geometry comes from.
#[derive(Debug, Deserialize)]
pub struct PartDef {
    /// Part name, referenced by [`Instance::part`].
    pub name: String,
    /// Path to the part's `.loon` or `.vcad` source, relative to the spec.
    pub source: PathBuf,
}

/// One posed instance. Field-for-field #844's `PosedInstanceEntry`.
#[derive(Debug, Deserialize)]
pub struct Instance {
    /// Instance name — what `--focus` and `--highlight` match against.
    pub name: String,
    /// Which [`PartDef`] this instantiates.
    pub part: String,
    /// Translation, mm.
    #[serde(default)]
    pub x: f64,
    /// Translation, mm.
    #[serde(default)]
    pub y: f64,
    /// Translation, mm.
    #[serde(default)]
    pub z: f64,
    /// Euler rotation about X, degrees.
    #[serde(default)]
    pub rx: f64,
    /// Euler rotation about Y, degrees.
    #[serde(default)]
    pub ry: f64,
    /// Euler rotation about Z, degrees.
    #[serde(default)]
    pub rz: f64,
    /// Exploded-view offset, mm, scaled by `--explode`.
    #[serde(default)]
    pub ex: f64,
    /// Exploded-view offset, mm, scaled by `--explode`.
    #[serde(default)]
    pub ey: f64,
    /// Exploded-view offset, mm, scaled by `--explode`.
    #[serde(default)]
    pub ez: f64,
}

impl Instance {
    /// This instance's pose, with the exploded offset applied at `factor`.
    fn transform3d(&self, factor: f64) -> vcad_ir::Transform3D {
        vcad_ir::Transform3D {
            translation: vcad_ir::Vec3::new(
                self.x + self.ex * factor,
                self.y + self.ey * factor,
                self.z + self.ez * factor,
            ),
            rotation: vcad_ir::Vec3::new(self.rx, self.ry, self.rz),
            scale: vcad_ir::Vec3::new(1.0, 1.0, 1.0),
        }
    }
}

/// Read a part source as `.vcad` IR JSON, evaluating `.loon` first. Mirrors
/// the binary's own input handling so a part renders the same whether it is
/// named on the command line or referenced from an assembly.
fn read_source(path: &Path) -> Result<String, String> {
    let raw =
        std::fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    if path.extension().and_then(|e| e.to_str()) != Some("loon") {
        return Ok(raw);
    }
    let doc = vcad_loon::eval_vcad(raw.trim(), path.parent())
        .map_err(|e| format!("{}: {e}", path.display()))?;
    serde_json::to_string(&doc).map_err(|e| format!("{}: serialize: {e}", path.display()))
}

/// Load `spec_path` and pose every instance, exploding by `factor`.
///
/// Each part source is evaluated once and reused across its instances — three
/// planets cost one evaluation. A part whose source yields several roots is
/// unioned into one solid, so an instance is always exactly one part.
pub fn load(spec_path: &Path, factor: f64) -> Result<Vec<PosedPart>, String> {
    let text = std::fs::read_to_string(spec_path)
        .map_err(|e| format!("read {}: {e}", spec_path.display()))?;
    let spec: AssemblySpec = serde_json::from_str(&text)
        .map_err(|e| format!("{}: {e}", spec_path.display()))?;

    if spec.instances.is_empty() {
        return Err(format!("{}: no instances", spec_path.display()));
    }
    let base = spec_path.parent().unwrap_or(Path::new("."));

    let sources: HashMap<&str, &PathBuf> = spec
        .parts
        .iter()
        .map(|p| (p.name.as_str(), &p.source))
        .collect();

    let mut geometry: HashMap<String, Vec<Solid>> = HashMap::new();
    let mut posed = Vec::with_capacity(spec.instances.len());

    for inst in &spec.instances {
        if !geometry.contains_key(&inst.part) {
            let source = sources.get(inst.part.as_str()).ok_or_else(|| {
                let mut known: Vec<&str> = sources.keys().copied().collect();
                known.sort_unstable();
                format!(
                    "instance '{}' references undefined part '{}' — defined parts: [{}]",
                    inst.name,
                    inst.part,
                    known.join(", ")
                )
            })?;
            let path = if source.is_absolute() {
                (*source).clone()
            } else {
                base.join(source)
            };
            let raw = read_source(&path)?;
            let scene = crate::evaluate_vcad(&raw)?;
            if scene.is_empty() {
                return Err(format!("part '{}' ({}) is empty", inst.part, path.display()));
            }
            geometry.insert(
                inst.part.clone(),
                scene.into_iter().map(|s| s.solid).collect(),
            );
        }

        let xf = crate::transform3d_to_kernel(&inst.transform3d(factor));
        for solid in &geometry[&inst.part] {
            posed.push(PosedPart {
                solid: solid.apply_transform(&xf),
                name: Some(inst.name.clone()),
            });
        }
    }
    Ok(posed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explode_offsets_scale_by_the_factor() {
        let i = Instance {
            name: "a".into(),
            part: "p".into(),
            x: 1.0,
            y: 2.0,
            z: 3.0,
            rx: 0.0,
            ry: 0.0,
            rz: 0.0,
            ex: 0.0,
            ey: 0.0,
            ez: 10.0,
        };
        // Factor 0 is the assembled view: the offset must not move anything.
        assert_eq!(i.transform3d(0.0).translation.z, 3.0);
        assert_eq!(i.transform3d(1.0).translation.z, 13.0);
        assert_eq!(i.transform3d(0.6).translation.z, 9.0);
        // Only the offset axes move.
        assert_eq!(i.transform3d(2.0).translation.x, 1.0);
        assert_eq!(i.transform3d(2.0).translation.y, 2.0);
    }
}
