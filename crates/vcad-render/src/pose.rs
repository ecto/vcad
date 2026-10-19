//! `--pose`: set joint states on a document before an ordinary still render.
//!
//! Posing is a document edit, not a render mode: the joint states are written
//! into the IR and the usual forward-kinematics placement does the rest, so
//! every style (line art, raster, `--raytrace`, `--photoreal`) poses the same
//! way the viewer and `--animate` do.

use vcad_ir::{Document, JointKind};

/// Parse `name=deg[,name=deg...]`.
pub fn parse_pose_spec(spec: &str) -> Result<Vec<(String, f64)>, String> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|entry| {
            let (name, value) = entry
                .split_once('=')
                .ok_or_else(|| format!("--pose entry '{entry}' is not name=deg"))?;
            let value: f64 = value
                .trim()
                .parse()
                .map_err(|_| format!("--pose entry '{entry}': '{value}' is not a number"))?;
            if !value.is_finite() {
                return Err(format!("--pose entry '{entry}' is not finite"));
            }
            Ok((name.trim().to_string(), value))
        })
        .collect()
}

/// Parse a pose file: `{"joint": deg, ...}`, or an `--animate` keyframe file
/// (`{"tracks": [{"joint": .., "keys": [{"t": .., "value": ..}]}]}`) whose
/// first key per joint track is the pose.
pub fn parse_pose_json(json: &str) -> Result<Vec<(String, f64)>, String> {
    let value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("invalid JSON: {e}"))?;
    let obj = value.as_object().ok_or("a pose file is a JSON object")?;
    if let Some(tracks) = obj.get("tracks") {
        let tracks = tracks.as_array().ok_or("`tracks` must be an array")?;
        let mut out = Vec::new();
        for t in tracks {
            let Some(joint) = t.get("joint").and_then(|j| j.as_str()) else {
                continue; // parameter tracks have no pose meaning
            };
            let first = t
                .get("keys")
                .and_then(|k| k.as_array())
                .and_then(|k| k.first())
                .and_then(|k| k.get("value"))
                .and_then(|v| v.as_f64())
                .ok_or_else(|| format!("track '{joint}' has no numeric first key"))?;
            out.push((joint.to_string(), first));
        }
        return Ok(out);
    }
    obj.iter()
        .map(|(k, v)| {
            v.as_f64()
                .map(|d| (k.clone(), d))
                .ok_or_else(|| format!("pose value for '{k}' is not a number"))
        })
        .collect()
}

/// Apply a pose to a document's joints, returning the re-serialized document
/// and any clamp warnings. Joints resolve by id, then by name; an unknown
/// joint is an error. States beyond declared limits are clamped.
pub fn apply_pose(raw: &str, pose: &[(String, f64)]) -> Result<(String, Vec<String>), String> {
    let mut doc: Document =
        serde_json::from_str(raw).map_err(|e| format!("--pose: cannot parse document: {e}"))?;
    let warnings = pose_document(&mut doc, pose)?;
    let out = serde_json::to_string(&doc).map_err(|e| format!("--pose: {e}"))?;
    Ok((out, warnings))
}

/// [`apply_pose`] on an already-parsed document.
pub fn pose_document(doc: &mut Document, pose: &[(String, f64)]) -> Result<Vec<String>, String> {
    let joints = doc.joints.get_or_insert_with(Vec::new);
    let mut warnings = Vec::new();
    for (name, value) in pose {
        let idx = joints
            .iter()
            .position(|j| &j.id == name)
            .or_else(|| joints.iter().position(|j| j.name.as_deref() == Some(name)))
            .ok_or_else(|| {
                let mut known: Vec<String> = joints
                    .iter()
                    .filter(|j| !matches!(j.kind, JointKind::Fixed))
                    .map(|j| j.name.clone().unwrap_or_else(|| j.id.clone()))
                    .collect();
                known.sort();
                format!(
                    "--pose: no joint '{name}'. Movable joints: {}",
                    if known.is_empty() {
                        "none".to_string()
                    } else {
                        known.join(", ")
                    }
                )
            })?;
        let j = &mut joints[idx];
        let limits = match &j.kind {
            JointKind::Revolute { limits, .. } | JointKind::Slider { limits, .. } => *limits,
            _ => None,
        };
        let mut v = *value;
        if let Some((lo, hi)) = limits {
            let c = v.clamp(lo.min(hi), hi.max(lo));
            if c != v {
                warnings.push(format!(
                    "joint '{name}' = {v} is outside its limits [{lo}, {hi}]; clamped to {c}"
                ));
                v = c;
            }
        }
        j.state = v;
    }
    Ok(warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_parses_and_rejects_garbage() {
        let p = parse_pose_spec("knee=30, hip = -12.5").unwrap();
        assert_eq!(p, vec![("knee".into(), 30.0), ("hip".into(), -12.5)]);
        assert!(parse_pose_spec("knee").is_err());
        assert!(parse_pose_spec("knee=abc").is_err());
    }

    #[test]
    fn json_takes_maps_and_keyframe_files() {
        let p = parse_pose_json(r#"{"a": 10, "b": -5}"#).unwrap();
        assert_eq!(p.len(), 2);
        let p = parse_pose_json(
            r#"{"durationS": 1, "tracks": [{"joint": "knee", "keys": [{"t": 0, "value": 70}, {"t": 1, "value": 0}]}]}"#,
        )
        .unwrap();
        assert_eq!(p, vec![("knee".into(), 70.0)]);
    }

    fn doc() -> String {
        serde_json::json!({
            "version": "0.1", "nodes": {}, "materials": {}, "part_materials": {}, "roots": [],
            "instances": [
                {"id": "a", "partDefId": "p"}, {"id": "b", "partDefId": "p"}
            ],
            "joints": [{
                "id": "j0", "name": "knee", "parentInstanceId": "a", "childInstanceId": "b",
                "parentAnchor": {"x": 0, "y": 0, "z": 0}, "childAnchor": {"x": 0, "y": 0, "z": 0},
                "kind": {"type": "Revolute", "axis": {"x": 0, "y": 1, "z": 0}, "limits": [-10, 120]},
                "state": 0
            }]
        })
        .to_string()
    }

    #[test]
    fn pose_sets_state_by_name_and_clamps() {
        let (out, w) = apply_pose(&doc(), &[("knee".into(), 45.0)]).unwrap();
        let d: Document = serde_json::from_str(&out).unwrap();
        assert_eq!(d.joints.unwrap()[0].state, 45.0);
        assert!(w.is_empty());

        let (out, w) = apply_pose(&doc(), &[("j0".into(), 200.0)]).unwrap();
        let d: Document = serde_json::from_str(&out).unwrap();
        assert_eq!(d.joints.unwrap()[0].state, 120.0);
        assert_eq!(w.len(), 1);

        let err = apply_pose(&doc(), &[("elbow".into(), 1.0)]).unwrap_err();
        assert!(err.contains("elbow") && err.contains("knee"), "{err}");
    }
}
