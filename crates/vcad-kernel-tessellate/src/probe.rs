//! Probe suites: batch material/void assertions over a posed assembly.
//!
//! A probe suite is a JSON file describing an assembly (named parts, each a
//! mesh plus a pose) and a list of assertions against it:
//!
//! ```json
//! {
//!   "parts": [
//!     { "name": "rotor-rear", "mesh": "rana-60c-rotor.stl",
//!       "pose": [{ "op": "translate", "xyz": [0, 0, 5.1] }] },
//!     { "name": "rotor-front", "mesh": "rana-60c-rotor.stl",
//!       "pose": [{ "op": "flip_x" },
//!                { "op": "rotate_z", "deg": 180 },
//!                { "op": "translate", "xyz": [0, 0, 23.5] }] }
//!   ],
//!   "probes": [
//!     { "name": "AIRGAP 1 band mean", "point": [22.1, 0.5, 10.8],
//!       "want_material": false, "parts": ["rotor-rear", "stator"] }
//!   ],
//!   "clearances": [
//!     { "name": "rotor faces", "a": "rotor-rear", "b": "rotor-front",
//!       "min": 0.5 }
//!   ]
//! }
//! ```
//!
//! `want_material` is asserted with *any-inside* semantics over `parts`: the
//! probe passes when the point is inside at least one listed part and
//! `want_material` is true, or inside none of them and it is false. That is
//! the shape rana's suites converged on, where a "void" claim only means
//! anything relative to the specific parts that could have filled it.
//!
//! Pose ops apply in the order written. Mesh paths resolve relative to the
//! probe file's directory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::query::{Assembly, Pose, QueryError};
use crate::TriangleMesh;

/// One placement step in a part's pose.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum PoseOp {
    /// Translate by `xyz`.
    Translate {
        /// Offset in mm.
        xyz: [f64; 3],
    },
    /// Rotate about Z by `deg` degrees.
    RotateZ {
        /// Angle in degrees.
        deg: f64,
    },
    /// Half-turn about the X axis: `(x, y, z) -> (x, -y, -z)`.
    FlipX,
}

impl PoseOp {
    fn to_pose(self) -> Pose {
        match self {
            PoseOp::Translate { xyz } => Pose::translate(xyz),
            PoseOp::RotateZ { deg } => Pose::rotate_z_deg(deg),
            PoseOp::FlipX => Pose::flip_x(),
        }
    }
}

/// A named instance in the probed assembly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartSpec {
    /// Instance name referenced by probes.
    pub name: String,
    /// Mesh source, relative to the probe file (binary STL).
    pub mesh: String,
    /// Placement steps, applied in order. Empty means identity.
    #[serde(default)]
    pub pose: Vec<PoseOp>,
}

/// One material/void assertion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeSpec {
    /// Human-readable assertion name, printed in the report.
    pub name: String,
    /// World coordinate to classify.
    pub point: [f64; 3],
    /// Expected: material (`true`) or void (`false`) across `parts`.
    pub want_material: bool,
    /// Instances the claim is made against (any-inside union).
    pub parts: Vec<String>,
}

/// One min/max clearance assertion between two instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClearanceSpec {
    /// Human-readable assertion name.
    pub name: String,
    /// First instance.
    pub a: String,
    /// Second instance.
    pub b: String,
    /// Lower bound in mm, if asserted.
    #[serde(default)]
    pub min: Option<f64>,
    /// Upper bound in mm, if asserted.
    #[serde(default)]
    pub max: Option<f64>,
}

/// A whole probe file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeSuite {
    /// Optional suite name for the report header.
    #[serde(default)]
    pub name: Option<String>,
    /// Named, posed instances.
    pub parts: Vec<PartSpec>,
    /// Material/void assertions.
    #[serde(default)]
    pub probes: Vec<ProbeSpec>,
    /// Clearance assertions.
    #[serde(default)]
    pub clearances: Vec<ClearanceSpec>,
}

/// Outcome of one assertion.
#[derive(Debug, Clone)]
pub struct ProbeOutcome {
    /// Assertion name.
    pub name: String,
    /// Did it hold?
    pub passed: bool,
    /// What was measured, phrased for a report line.
    pub detail: String,
}

/// Result of running a suite.
#[derive(Debug, Clone, Default)]
pub struct ProbeReport {
    /// Every assertion, in file order (probes then clearances).
    pub outcomes: Vec<ProbeOutcome>,
}

impl ProbeReport {
    /// Count of passing assertions.
    pub fn passed(&self) -> usize {
        self.outcomes.iter().filter(|o| o.passed).count()
    }

    /// Count of failing assertions.
    pub fn failed(&self) -> usize {
        self.outcomes.len() - self.passed()
    }

    /// True when every assertion held. CI gates on this.
    pub fn ok(&self) -> bool {
        self.failed() == 0
    }

    /// A plain-text report: one line per assertion plus a tally.
    pub fn render(&self) -> String {
        let mut s = String::new();
        for o in &self.outcomes {
            s.push_str(&format!(
                "  {} {}: {}\n",
                if o.passed { "PASS" } else { "FAIL" },
                o.name,
                o.detail
            ));
        }
        s.push_str(&format!(
            "{} passed, {} failed, {} total\n",
            self.passed(),
            self.failed(),
            self.outcomes.len()
        ));
        s
    }
}

/// Anything that went wrong loading or running a suite.
#[derive(Debug)]
pub enum ProbeError {
    /// The probe file could not be read or parsed.
    Io(String),
    /// A mesh referenced by a part could not be loaded.
    Mesh(String),
    /// The assembly rejected a part or a query.
    Query(QueryError),
}

impl std::fmt::Display for ProbeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProbeError::Io(m) | ProbeError::Mesh(m) => write!(f, "{m}"),
            ProbeError::Query(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ProbeError {}

impl From<QueryError> for ProbeError {
    fn from(e: QueryError) -> Self {
        ProbeError::Query(e)
    }
}

/// Build the posed assembly a suite describes, resolving each part's mesh
/// through `load` (called once per distinct mesh reference).
pub fn build_assembly<F>(suite: &ProbeSuite, mut load: F) -> Result<Assembly, ProbeError>
where
    F: FnMut(&str) -> Result<TriangleMesh, ProbeError>,
{
    let mut cache: HashMap<String, TriangleMesh> = HashMap::new();
    let mut asm = Assembly::new();
    for part in &suite.parts {
        if !cache.contains_key(&part.mesh) {
            cache.insert(part.mesh.clone(), load(&part.mesh)?);
        }
        let pose = part
            .pose
            .iter()
            .fold(Pose::identity(), |acc, op| acc.then(&op.to_pose()));
        asm.insert(part.name.clone(), &cache[&part.mesh], pose)?;
    }
    Ok(asm)
}

/// Run every assertion in `suite` against `asm`.
pub fn run_suite(suite: &ProbeSuite, asm: &Assembly) -> Result<ProbeReport, ProbeError> {
    let mut report = ProbeReport::default();
    for p in &suite.probes {
        let got = asm.any_inside(&p.parts, p.point)?;
        let word = |m: bool| if m { "material" } else { "void" };
        report.outcomes.push(ProbeOutcome {
            name: p.name.clone(),
            passed: got == p.want_material,
            detail: format!(
                "want {} at ({:.3}, {:.3}, {:.3}), got {} ({})",
                word(p.want_material),
                p.point[0],
                p.point[1],
                p.point[2],
                word(got),
                p.parts.join(", ")
            ),
        });
    }
    for c in &suite.clearances {
        let r = asm.clearance(&c.a, &c.b)?;
        let lo_ok = c.min.is_none_or(|m| r.distance >= m);
        let hi_ok = c.max.is_none_or(|m| r.distance <= m);
        report.outcomes.push(ProbeOutcome {
            name: c.name.clone(),
            passed: lo_ok && hi_ok,
            detail: format!(
                "{} <-> {}: {:.4} mm{}{}{}",
                c.a,
                c.b,
                r.distance,
                if r.intersecting {
                    " (intersecting)"
                } else {
                    ""
                },
                c.min.map(|m| format!(", min {m}")).unwrap_or_default(),
                c.max.map(|m| format!(", max {m}")).unwrap_or_default(),
            ),
        });
    }
    Ok(report)
}

/// Load a probe file, resolve its meshes as binary STL relative to the file's
/// directory, and run it.
pub fn run_probe_file(path: &Path) -> Result<ProbeReport, ProbeError> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| ProbeError::Io(format!("{}: {e}", path.display())))?;
    let suite: ProbeSuite = serde_json::from_str(&text)
        .map_err(|e| ProbeError::Io(format!("{}: {e}", path.display())))?;
    let base: PathBuf = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let asm = build_assembly(&suite, |rel| {
        let p = base.join(rel);
        let bytes =
            std::fs::read(&p).map_err(|e| ProbeError::Mesh(format!("{}: {e}", p.display())))?;
        parse_binary_stl(&bytes).map_err(|e| ProbeError::Mesh(format!("{}: {e}", p.display())))
    })?;
    run_suite(&suite, &asm)
}

/// Parse a binary STL into a [`TriangleMesh`] (one unshared vertex triple per
/// facet — parity classification and clearance never need welded vertices).
pub fn parse_binary_stl(bytes: &[u8]) -> Result<TriangleMesh, String> {
    if bytes.len() < 84 {
        return Err("not a binary STL (under 84 bytes)".into());
    }
    let count = u32::from_le_bytes([bytes[80], bytes[81], bytes[82], bytes[83]]) as usize;
    let need = 84 + count * 50;
    if bytes.len() < need {
        return Err(format!(
            "truncated binary STL: {count} facets need {need} bytes, file has {}",
            bytes.len()
        ));
    }
    let f32_at =
        |o: usize| f32::from_le_bytes([bytes[o], bytes[o + 1], bytes[o + 2], bytes[o + 3]]);
    let mut vertices = Vec::with_capacity(count * 9);
    let mut normals = Vec::with_capacity(count * 9);
    let mut indices = Vec::with_capacity(count * 3);
    for i in 0..count {
        let rec = 84 + i * 50;
        let n = [f32_at(rec), f32_at(rec + 4), f32_at(rec + 8)];
        for c in 0..3 {
            let v = rec + 12 + c * 12;
            vertices.extend_from_slice(&[f32_at(v), f32_at(v + 4), f32_at(v + 8)]);
            normals.extend_from_slice(&n);
            indices.push((i * 3 + c) as u32);
        }
    }
    Ok(TriangleMesh {
        vertices,
        indices,
        normals,
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::tests::box_mesh;

    fn stl_bytes(mesh: &TriangleMesh) -> Vec<u8> {
        let n = mesh.indices.len() / 3;
        let mut b = vec![0u8; 80];
        b.extend_from_slice(&(n as u32).to_le_bytes());
        for t in mesh.indices.chunks_exact(3) {
            b.extend_from_slice(&[0u8; 12]);
            for &i in t {
                for k in 0..3 {
                    b.extend_from_slice(&mesh.vertices[3 * i as usize + k].to_le_bytes());
                }
            }
            b.extend_from_slice(&[0u8; 2]);
        }
        b
    }

    #[test]
    fn stl_round_trips_through_the_parser() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let parsed = parse_binary_stl(&stl_bytes(&m)).unwrap();
        assert_eq!(parsed.indices.len(), m.indices.len());
        assert!(crate::query::is_inside(&parsed, [5.0, 5.1, 4.9]));
        assert!(!crate::query::is_inside(&parsed, [5.0, 5.1, 12.0]));
    }

    #[test]
    fn suite_reports_pass_and_fail() {
        let m = box_mesh([0.0, 0.0, 0.0], [10.0, 10.0, 10.0]);
        let suite: ProbeSuite = serde_json::from_str(
            r#"{
              "parts": [
                {"name": "lower", "mesh": "box.stl"},
                {"name": "upper", "mesh": "box.stl",
                 "pose": [{"op": "translate", "xyz": [0, 0, 12]}]}
              ],
              "probes": [
                {"name": "solid", "point": [5, 5.1, 4.9],
                 "want_material": true, "parts": ["lower", "upper"]},
                {"name": "gap is void", "point": [5, 5.1, 11],
                 "want_material": true, "parts": ["lower", "upper"]}
              ],
              "clearances": [
                {"name": "stack gap", "a": "lower", "b": "upper", "min": 1.9, "max": 2.1}
              ]
            }"#,
        )
        .unwrap();
        let asm = build_assembly(&suite, |_| Ok(m.clone())).unwrap();
        let report = run_suite(&suite, &asm).unwrap();
        assert_eq!(report.outcomes.len(), 3);
        assert!(report.outcomes[0].passed);
        assert!(
            !report.outcomes[1].passed,
            "the 2mm gap is void, not material"
        );
        assert!(report.outcomes[2].passed, "{}", report.render());
        assert!(!report.ok());
        assert_eq!(report.failed(), 1);
    }
}
