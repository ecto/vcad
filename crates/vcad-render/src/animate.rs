//! Batch (animation) rendering for the photoreal path.
//!
//! A jointed assembly posed over time is, geometrically, the *same* parts in
//! different places. The expensive half of a photoreal render — evaluating
//! the parametric DAG into BReps and building a BVH over each one — depends
//! only on the parts, so it belongs outside the frame loop. This module puts
//! it there: evaluate once, build one BLAS per part, then per frame solve
//! forward kinematics, hand each object its new object→world transform, and
//! rebuild only the top-level acceleration structure before tracing.
//!
//! The alternative — which this replaces — is to bake one document per frame
//! and shell out to the renderer N times, paying the full evaluation cost
//! every frame. On a 35-part machine at 640px/16spp that was ~55 s a frame
//! against ~10 s of actual tracing.
//!
//! The pose source is the document's own [`Timeline`] when it has one, or a
//! keyframe file passed to `--animate`. Both are sampled by
//! [`Timeline::sample_sequence`], the same code the MCP `animate` tool and
//! the app's timeline scrubber use, so a rendered frame and a scrubbed frame
//! agree by construction.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vcad_ir::animation::{AnimTarget, Timeline};

use crate::photoreal::{
    build_objects, check_raster_opts, dress_scene, frame_view, object_corners, object_corners_with,
    trace_frame, PhotorealOptions,
};
use crate::raster::encode_png;
use crate::RasterOptions;

/// What to render, and where to put it.
#[derive(Debug, Clone)]
pub struct AnimateOptions {
    /// Directory the `frame_NNNN.png` files are written to. Created if absent.
    pub out_dir: PathBuf,
    /// Keyframes to drive the assembly with. `None` uses the document's own
    /// timeline, which is where the MCP `animate` tool leaves one.
    pub timeline: Option<Timeline>,
    /// Render only the first `n` frames of the sampled sequence. `None`
    /// renders the whole timeline.
    pub frames: Option<usize>,
    /// Override the timeline's own frame rate.
    pub fps: Option<f64>,
}

/// What a finished animation cost, frame by frame.
#[derive(Debug, Clone)]
pub struct AnimateReport {
    /// The PNG files written, in order.
    pub frames: Vec<PathBuf>,
    /// Effective frame rate (the timeline's, or the `--fps` override).
    pub fps: f64,
    /// Time spent evaluating the document — paid exactly once.
    pub eval: Duration,
    /// Time spent building per-part BVHs and framing the camera — also once.
    pub setup: Duration,
    /// Per-frame trace + encode + write time.
    pub per_frame: Vec<Duration>,
    /// Number of traceable parts in the scene.
    pub parts: usize,
    /// Number of those parts driven by joints (the rest are static).
    pub articulated: usize,
}

impl AnimateReport {
    /// Total wall time across evaluation, setup, and every frame.
    pub fn total(&self) -> Duration {
        self.eval + self.setup + self.per_frame.iter().sum::<Duration>()
    }

    /// Median per-frame cost — the steady-state number, unpolluted by a
    /// first frame that warms caches.
    pub fn median_frame(&self) -> Duration {
        if self.per_frame.is_empty() {
            return Duration::ZERO;
        }
        let mut v = self.per_frame.clone();
        v.sort_unstable();
        v[v.len() / 2]
    }
}

/// Parse an `--animate` keyframe file.
///
/// Accepts the document's own [`Timeline`] shape verbatim, and additionally a
/// shorthand for the common case of driving joints:
///
/// ```json
/// { "fps": 24, "durationS": 6,
///   "tracks": [ { "joint": "A", "keys": [ { "t": 0, "value": 0 },
///                                         { "t": 6, "value": 1440 } ] } ] }
/// ```
///
/// `"joint"` may name a joint by id (`joint_0`) or by its authored name
/// (`A`); resolution against the document happens later, in
/// [`render_photoreal_animation`], so a typo fails with the list of joints
/// that do exist rather than silently animating nothing.
pub fn parse_timeline_spec(json: &str) -> Result<Timeline, String> {
    let mut value: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("--animate: not valid JSON: {e}"))?;

    // Rewrite the shorthand into real `AnimTarget`s before deserializing, so
    // the IR type stays the single definition of a track.
    if let Some(tracks) = value.get_mut("tracks").and_then(|t| t.as_array_mut()) {
        for track in tracks {
            let Some(obj) = track.as_object_mut() else {
                continue;
            };
            if obj.contains_key("target") {
                continue;
            }
            let sugar = ["joint", "parameter", "visibility"]
                .into_iter()
                .find_map(|k| obj.remove(k).map(|v| (k, v)));
            let target = match sugar {
                Some(("joint", v)) => serde_json::json!({ "type": "Joint", "jointId": v }),
                Some(("parameter", v)) => serde_json::json!({ "type": "Parameter", "name": v }),
                Some(("visibility", v)) => {
                    serde_json::json!({ "type": "Visibility", "instanceId": v })
                }
                _ => {
                    return Err("--animate: each track needs a `target`, or the \
                                `joint`/`parameter`/`visibility` shorthand"
                        .to_string())
                }
            };
            obj.insert("target".to_string(), target);
        }
    }

    let timeline: Timeline =
        serde_json::from_value(value).map_err(|e| format!("--animate: not a timeline: {e}"))?;
    if !(timeline.duration_s.is_finite() && timeline.duration_s > 0.0) {
        return Err("--animate: durationS must be positive".to_string());
    }
    if timeline.tracks.is_empty() {
        return Err("--animate: timeline has no tracks — nothing would move".to_string());
    }
    Ok(timeline)
}

/// Resolve a track's joint reference against the document: joint id first,
/// then authored joint name. Fails closed, listing what is available.
fn resolve_joint<'a>(
    doc: &'a vcad_ir::Document,
    reference: &str,
) -> Result<&'a vcad_ir::Joint, String> {
    let joints = doc.joints.as_deref().unwrap_or(&[]);
    if let Some(j) = joints.iter().find(|j| j.id == reference) {
        return Ok(j);
    }
    if let Some(j) = joints.iter().find(|j| j.name.as_deref() == Some(reference)) {
        return Ok(j);
    }
    let mut known: Vec<String> = joints
        .iter()
        .map(|j| match &j.name {
            Some(n) => format!("{} ({n})", j.id),
            None => j.id.clone(),
        })
        .collect();
    known.sort();
    Err(format!(
        "--animate: no joint '{reference}' in the document. Available: {}",
        if known.is_empty() {
            "none — this document has no joints".to_string()
        } else {
            known.join(", ")
        }
    ))
}

/// Render a jointed assembly over a timeline, evaluating geometry once.
///
/// Writes `frame_0000.png` … into `an.out_dir` and reports what each stage
/// cost. `progress` is called after every frame with `(index, total,
/// elapsed)` so a CLI can stream timings.
pub fn render_photoreal_animation(
    raw_vcad: &str,
    opts: &RasterOptions,
    pr: &PhotorealOptions,
    an: &AnimateOptions,
    progress: &mut dyn FnMut(usize, usize, Duration),
) -> Result<AnimateReport, String> {
    check_raster_opts(opts)?;

    // ── evaluate, exactly once ───────────────────────────────────────────
    let t_eval = Instant::now();
    let evals_before = crate::document_eval_count();
    let mut ev = crate::evaluate_vcad_document(raw_vcad)?;
    let eval = t_eval.elapsed();

    if ev.parts.is_empty() {
        return Err(
            "--animate: this document has no assembly instances; only a \
                    jointed assembly can be posed over time"
                .to_string(),
        );
    }

    let timeline = match an.timeline.clone().or_else(|| ev.doc.timeline.clone()) {
        Some(t) => t,
        None => {
            return Err(
                "--animate: no timeline. Pass --animate <keyframes.json>, or \
                        set one on the document"
                    .to_string(),
            )
        }
    };
    let timeline = Timeline {
        fps: an.fps.filter(|f| *f > 0.0).unwrap_or(timeline.fps),
        ..timeline
    };

    // Fail closed on a track that names a joint the document doesn't have,
    // and record the id→state mapping the sampler's keys resolve to.
    let mut track_joint_ids: Vec<(String, String)> = Vec::new();
    for track in &timeline.tracks {
        if let AnimTarget::Joint { joint_id } = &track.target {
            let j = resolve_joint(&ev.doc, joint_id)?;
            track_joint_ids.push((joint_id.clone(), j.id.clone()));
        }
    }
    if track_joint_ids.is_empty() {
        return Err("--animate: no joint tracks. Parameter tracks would need \
                    per-frame re-evaluation, which this path deliberately does not do"
            .to_string());
    }

    let t_setup = Instant::now();

    // ── one BLAS per part, built once ────────────────────────────────────
    // Static scene roots first (their transforms never change), then the
    // assembly instances, whose object→world transform is rewritten per
    // frame.
    let mut objects = build_objects(&ev.statics).unwrap_or_default();
    let static_count = objects.len();

    let part_solids: Vec<crate::SceneSolid> = std::mem::take(&mut ev.parts)
        .into_iter()
        .map(crate::part_as_local_scene_solid)
        .collect();
    let instance_ids: Vec<String> = part_solids.iter().map(|s| s.id.clone()).collect();
    objects.extend(build_objects(&part_solids)?);
    let articulated = objects.len() - static_count;
    // build_objects fails closed on untraceable parts, so counts can only
    // agree here; keep the invariant visible without implying a live branch.
    debug_assert_eq!(
        articulated,
        instance_ids.len(),
        "build_objects returned fewer objects than instances without erroring"
    );

    // ── sample the timeline, and pose every frame up front ───────────────
    let mut sequence = timeline.sample_sequence();
    if let Some(n) = an.frames {
        if n == 0 {
            return Err("--frames must be at least 1".to_string());
        }
        sequence.truncate(n);
    }

    let fallbacks: std::collections::HashMap<&str, Option<vcad_ir::Transform3D>> = ev
        .doc
        .instances
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(|i| (i.id.as_str(), i.transform))
        .collect();

    let mut poses: Vec<Vec<vcad_kernel::vcad_kernel_math::Transform>> =
        Vec::with_capacity(sequence.len());
    for frame in &sequence {
        // Drive the document's joints, then let the shared FK solver do the
        // rest — chained fixed joints carry their groups along for free.
        if let Some(joints) = ev.doc.joints.as_mut() {
            for (reference, id) in &track_joint_ids {
                let Some(value) = frame.joints.get(reference) else {
                    continue;
                };
                if let Some(j) = joints.iter_mut().find(|j| &j.id == id) {
                    j.state = *value;
                }
            }
        }
        let world = crate::assembly_world_transforms(&ev.doc)?;
        poses.push(
            instance_ids
                .iter()
                .map(|id| {
                    let t = world
                        .get(id)
                        .copied()
                        .or_else(|| fallbacks.get(id.as_str()).copied().flatten());
                    match t {
                        Some(t) => crate::transform3d_to_kernel(&t),
                        None => vcad_kernel::vcad_kernel_math::Transform::identity(),
                    }
                })
                .collect(),
        );
    }

    // ── frame the camera on the union of every pose ──────────────────────
    // A per-frame fit would make the machine swim about in the canvas; the
    // union keeps the camera locked and nothing ever leaves the frame.
    let mut corners: Vec<[f64; 3]> = objects[..static_count]
        .iter()
        .flat_map(object_corners)
        .collect();
    for pose in &poses {
        for (obj, t) in objects[static_count..].iter().zip(pose) {
            // Bounds under this pose, WITHOUT writing the pose onto the
            // object — the trace loop owns those transforms.
            corners.extend(object_corners_with(obj, t));
        }
    }
    let framing = frame_view(&corners, opts, pr)?;
    let setup = t_setup.elapsed();

    std::fs::create_dir_all(&an.out_dir)
        .map_err(|e| format!("create {}: {e}", an.out_dir.display()))?;

    // ── trace ────────────────────────────────────────────────────────────
    let mut report = AnimateReport {
        frames: Vec::with_capacity(sequence.len()),
        fps: timeline.fps,
        eval,
        setup,
        per_frame: Vec::with_capacity(sequence.len()),
        parts: objects.len(),
        articulated,
    };

    let total = sequence.len();
    // `Scene` owns its objects, so hand it a fresh vector each frame built
    // from the same `Arc<Bvh>` handles — an Arc bump per part, not a rebuild.
    let blueprint: Vec<(std::sync::Arc<vcad_kernel_raytrace::Bvh>, _)> = objects
        .iter()
        .map(|o| (std::sync::Arc::clone(&o.bvh), o.material))
        .collect();
    let static_transforms: Vec<_> = objects[..static_count]
        .iter()
        .map(|o| o.transform.clone())
        .collect();

    for (i, pose) in poses.iter().enumerate() {
        let t_frame = Instant::now();
        let frame_objects: Vec<vcad_kernel_raytrace::pathtrace::Object> = blueprint
            .iter()
            .enumerate()
            .map(|(k, (bvh, material))| {
                let transform = if k < static_count {
                    static_transforms[k].clone()
                } else {
                    pose[k - static_count].clone()
                };
                vcad_kernel_raytrace::pathtrace::Object::placed(
                    std::sync::Arc::clone(bvh),
                    *material,
                    transform,
                )
            })
            .collect();
        let scene = dress_scene(frame_objects, &framing, pr)?;
        let png = encode_png(trace_frame(&scene, &framing, pr, true), opts)?;
        let path = an.out_dir.join(format!("frame_{i:04}.png"));
        std::fs::write(&path, png).map_err(|e| format!("write {}: {e}", path.display()))?;
        let elapsed = t_frame.elapsed();
        report.frames.push(path);
        report.per_frame.push(elapsed);
        progress(i + 1, total, elapsed);
    }

    // The whole point of this path, asserted rather than asserted-to.
    let evals = crate::document_eval_count() - evals_before;
    debug_assert_eq!(evals, 1, "geometry was evaluated {evals} times, not once");

    Ok(report)
}

/// Assemble `report`'s frames into an H.264 mp4 with ffmpeg.
///
/// Returns the ffmpeg command line when ffmpeg is not on `PATH` (or fails),
/// so a caller can print it rather than losing the frames. Deliberately not
/// load-bearing: the PNGs are the deliverable, the mp4 is a convenience.
pub fn assemble_mp4(frame_dir: &Path, fps: f64, out: &Path) -> Result<(), String> {
    let args: Vec<String> = vec![
        "-y".into(),
        "-framerate".into(),
        format!("{fps}"),
        "-i".into(),
        frame_dir.join("frame_%04d.png").display().to_string(),
        "-vf".into(),
        "pad=ceil(iw/2)*2:ceil(ih/2)*2".into(),
        "-c:v".into(),
        "libx264".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        out.display().to_string(),
    ];
    let rendered = format!("ffmpeg {}", args.join(" "));
    match std::process::Command::new("ffmpeg")
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("ffmpeg exited {s}; run it yourself:\n  {rendered}")),
        Err(_) => Err(format!(
            "ffmpeg not found on PATH; the frames are written — run:\n  {rendered}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::animation::{AnimKey, AnimTrack};

    /// Two cubes, one bolted to the other by `kind`, driven from a timeline.
    fn jointed_doc(kind: &str, axis: [f64; 3]) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{ "id": 1, "name": "base", "op": {{ "type": "Cube", "size": {{ "x": 20, "y": 20, "z": 4 }} }} }},
    "2": {{ "id": 2, "name": "arm", "op": {{ "type": "Cube", "size": {{ "x": 40, "y": 6, "z": 6 }} }} }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [],
  "partDefs": {{
    "base": {{ "id": "base", "name": "base", "root": 1 }},
    "arm": {{ "id": "arm", "name": "arm", "root": 2 }}
  }},
  "instances": [
    {{ "id": "base", "partDefId": "base" }},
    {{ "id": "arm", "partDefId": "arm" }}
  ],
  "joints": [
    {{ "id": "joint_0", "name": "swing", "parentInstanceId": "base",
       "childInstanceId": "arm",
       "parentAnchor": {{ "x": 0, "y": 0, "z": 0 }},
       "childAnchor": {{ "x": 0, "y": 0, "z": 0 }},
       "kind": {{ "type": "{kind}", "axis": {{ "x": {}, "y": {}, "z": {} }}, "limits": [-1000, 1000] }},
       "state": 0.0 }}
  ],
  "groundInstanceId": "base"
}}"#,
            axis[0], axis[1], axis[2]
        )
    }

    fn joint_timeline(id: &str, from: f64, to: f64) -> Timeline {
        Timeline {
            duration_s: 1.0,
            fps: 2.0,
            tracks: vec![AnimTrack {
                target: AnimTarget::Joint {
                    joint_id: id.to_string(),
                },
                keys: vec![
                    AnimKey {
                        t: 0.0,
                        value: from,
                        ease: Default::default(),
                    },
                    AnimKey {
                        t: 1.0,
                        value: to,
                        ease: Default::default(),
                    },
                ],
            }],
            camera: Vec::new(),
        }
    }

    /// The world pose of a revolute child must actually rotate with the
    /// joint state, about the anchor — the FK contract the animation path
    /// leans on for every frame after the first.
    #[test]
    fn revolute_state_swings_the_child() {
        let doc: vcad_ir::Document =
            serde_json::from_str(&jointed_doc("Revolute", [0.0, 0.0, 1.0])).expect("parse");
        let pose_at = |deg: f64| {
            let mut doc = doc.clone();
            doc.joints.as_mut().unwrap()[0].state = deg;
            crate::assembly_world_transforms(&doc).expect("fk")["arm"]
        };
        assert!(
            pose_at(0.0).rotation.z.abs() < 1e-9,
            "zero state must be identity"
        );
        let quarter = pose_at(90.0);
        assert!(
            (quarter.rotation.z - 90.0).abs() < 1e-6,
            "expected a 90° Z rotation, got {:?}",
            quarter.rotation
        );
        // Anchor at the origin, so the child stays pinned there.
        assert!(quarter.translation.x.abs() < 1e-9);
        assert!(quarter.translation.y.abs() < 1e-9);
    }

    /// And a slider must translate along its axis by exactly the state.
    #[test]
    fn prismatic_state_slides_the_child() {
        let doc: vcad_ir::Document =
            serde_json::from_str(&jointed_doc("Slider", [1.0, 0.0, 0.0])).expect("parse");
        let mut doc = doc.clone();
        doc.joints.as_mut().unwrap()[0].state = 7.5;
        let pose = crate::assembly_world_transforms(&doc).expect("fk")["arm"];
        assert!(
            (pose.translation.x - 7.5).abs() < 1e-9,
            "got {:?}",
            pose.translation
        );
        assert!(pose.translation.y.abs() < 1e-9);
        assert!(pose.rotation.z.abs() < 1e-9, "a slider must not rotate");
    }

    #[test]
    fn shorthand_and_native_tracks_parse_the_same() {
        let sugar = parse_timeline_spec(
            r#"{"fps":24,"durationS":2,"tracks":[{"joint":"A","keys":[{"t":0,"value":0},{"t":2,"value":90}]}]}"#,
        )
        .expect("shorthand");
        let native = parse_timeline_spec(
            r#"{"fps":24,"durationS":2,"tracks":[{"target":{"type":"Joint","jointId":"A"},
                "keys":[{"t":0,"value":0},{"t":2,"value":90}]}]}"#,
        )
        .expect("native");
        assert_eq!(sugar, native);
    }

    #[test]
    fn a_track_naming_no_joint_fails_with_the_list() {
        let doc: vcad_ir::Document =
            serde_json::from_str(&jointed_doc("Revolute", [0.0, 0.0, 1.0])).expect("parse");
        let err = resolve_joint(&doc, "nope").expect_err("should fail");
        assert!(err.contains("joint_0"), "unhelpful error: {err}");
        assert!(
            err.contains("swing"),
            "should offer the authored name: {err}"
        );
    }

    /// A joint track may name its joint by authored name as well as by id —
    /// `.loon` authors write the name, the IR mints the id.
    #[test]
    fn joints_resolve_by_name_or_id() {
        let doc: vcad_ir::Document =
            serde_json::from_str(&jointed_doc("Revolute", [0.0, 0.0, 1.0])).expect("parse");
        assert_eq!(resolve_joint(&doc, "swing").unwrap().id, "joint_0");
        assert_eq!(resolve_joint(&doc, "joint_0").unwrap().id, "joint_0");
    }

    /// The timeline sampler is shared with the app; check the animation
    /// path's assumptions about it hold (inclusive of both ends, linear).
    #[test]
    fn sequence_covers_both_ends() {
        let tl = joint_timeline("joint_0", 0.0, 100.0);
        let seq = tl.sample_sequence();
        assert_eq!(seq.len(), 3, "1 s at 2 fps is t=0, 0.5, 1.0");
        assert_eq!(seq[0].joints["joint_0"], 0.0);
        assert_eq!(seq[1].joints["joint_0"], 50.0);
        assert_eq!(seq[2].joints["joint_0"], 100.0);
    }
}
