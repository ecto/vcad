//! Batch (animation) rendering: photoreal, ray-traced, and drafting.
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
//! That argument is about *geometry*, not about shading, so the same
//! front half serves all three styles: the drafting line-art raster
//! ([`render_drafting_animation`], the default), direct BRep ray tracing
//! ([`render_raytrace_animation`]), and the path tracer
//! ([`render_photoreal_animation`]). [`plan_animation`] is that front half —
//! resolve the timeline, evaluate once, sample the sequence, bake one pose
//! vector per frame — and each back end differs only in how it turns a pose
//! into pixels.
//!
//! Every back end frames its camera on the union of all posed bounds, once,
//! and hands that same framing to every frame (see
//! [`crate::FixedFraming`]). Framing per frame would resize the canvas as
//! the subject's projected extent changed — frames that no muxer will accept
//! and, even at a fixed size, a machine that swims about in the canvas.
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
use vcad_kernel::Solid;

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

/// A document evaluated once and posed for every frame — everything the
/// three back ends share, and the reason none of them re-evaluates geometry.
struct AnimationPlan {
    /// Static scene roots, already world-placed. Never move.
    statics: Vec<crate::SceneSolid>,
    /// Assembly instances in their *local* frame, in pose-vector order.
    parts: Vec<crate::SceneSolid>,
    /// `poses[frame][part]` — the object→world transform for `parts[part]`.
    poses: Vec<Vec<vcad_kernel::vcad_kernel_math::Transform>>,
    /// Effective frame rate (the timeline's, or the `--fps` override).
    fps: f64,
    /// What the single evaluation cost.
    eval: Duration,
}

/// Resolve the timeline, evaluate the document **once**, and bake one pose
/// vector per frame. The style-agnostic half of every animation render.
fn plan_animation(raw_vcad: &str, an: &AnimateOptions) -> Result<AnimationPlan, String> {
    // ── evaluate, exactly once ───────────────────────────────────────────
    let t_eval = Instant::now();
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

    let part_solids: Vec<crate::SceneSolid> = std::mem::take(&mut ev.parts)
        .into_iter()
        .map(crate::part_as_local_scene_solid)
        .collect();
    let instance_ids: Vec<String> = part_solids.iter().map(|s| s.id.clone()).collect();

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

    Ok(AnimationPlan {
        statics: std::mem::take(&mut ev.statics),
        parts: part_solids,
        poses,
        fps: timeline.fps,
        eval,
    })
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
    let evals_before = crate::document_eval_count();
    let plan = plan_animation(raw_vcad, an)?;
    let AnimationPlan {
        statics,
        parts: part_solids,
        poses,
        fps,
        eval,
    } = plan;

    let t_setup = Instant::now();

    // ── one BLAS per part, built once ────────────────────────────────────
    // Static scene roots first (their transforms never change), then the
    // assembly instances, whose object→world transform is rewritten per
    // frame.
    let mut objects = build_objects(&statics, pr.mesh_segments()).unwrap_or_default();
    let static_count = objects.len();
    objects.extend(build_objects(&part_solids, pr.mesh_segments())?);
    let articulated = objects.len() - static_count;
    // build_objects fails closed on untraceable parts, so counts can only
    // agree here; keep the invariant visible without implying a live branch.
    debug_assert_eq!(
        articulated,
        part_solids.len(),
        "build_objects returned fewer objects than instances without erroring"
    );

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
        frames: Vec::with_capacity(poses.len()),
        fps,
        eval,
        setup,
        per_frame: Vec::with_capacity(poses.len()),
        parts: objects.len(),
        articulated,
    };

    let total = poses.len();
    // `Scene` owns its objects, so hand it a fresh vector each frame built
    // from the same `Arc<Bvh>` handles — an Arc bump per part, not a rebuild.
    let blueprint: Vec<(std::sync::Arc<vcad_kernel_raytrace::Bvh>, _)> = objects
        .iter()
        .map(|o| (std::sync::Arc::clone(&o.bvh), o.material))
        .collect();
    // Back into the kernel's `Transform`, so the static placements and the
    // per-frame poses are one type the loop below can choose between.
    let static_transforms: Vec<vcad_kernel::vcad_kernel_math::Transform> = objects[..static_count]
        .iter()
        .map(|o| vcad_kernel::vcad_kernel_math::Transform {
            matrix: o.transform.matrix,
        })
        .collect();

    for (i, pose) in poses.iter().enumerate() {
        let t_frame = Instant::now();
        let frame_objects: Vec<vcad_kernel_raytrace::pathtrace::Object> = blueprint
            .iter()
            .enumerate()
            .map(|(k, (bvh, material))| {
                let transform = if k < static_count {
                    &static_transforms[k]
                } else {
                    &pose[k - static_count]
                };
                vcad_kernel_raytrace::pathtrace::Object::placed(
                    std::sync::Arc::clone(bvh),
                    *material,
                    vcad_kernel_raytrace::tlas::placement(transform),
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

/// Options the raster animation tiers cannot honour, refused up front with
/// the flag to drop.
///
/// `--trim` is the interesting one: it crops each frame to the drawn
/// content, which for a moving subject is a *different* crop every frame —
/// exactly the size drift a fixed camera exists to prevent. There is no
/// silently-correct interpretation, so it is refused rather than quietly
/// ignored.
fn check_animation_raster_opts(opts: &RasterOptions, tier: &str) -> Result<(), String> {
    if opts.size_px < 16 {
        return Err("size_px too small".to_string());
    }
    if !(opts.fill_frac > 0.0 && opts.fill_frac <= 1.0) {
        return Err("fill_frac must be in (0, 1]".to_string());
    }
    if opts.trim_margin_px.is_some() {
        return Err(format!(
            "--trim does not compose with --animate: it crops each frame to \
             its own content, so a moving subject would give every frame a \
             different size. Drop --trim (use --auto-aspect, which the {tier} \
             animation resolves once against the whole sequence)."
        ));
    }
    if opts.focus.is_some() {
        return Err(
            "--focus does not compose with --animate: the camera is framed \
             once on the whole posed sequence, so a per-part frame would be \
             overwritten. Drop --focus."
                .to_string(),
        );
    }
    Ok(())
}

/// Freeze a solid at `segments` into a mesh-backed solid.
///
/// The animation draws the same geometry in a new place every frame, so
/// tessellating inside the frame loop would pay the BRep→triangles cost N
/// times over for an identical result. A mesh-backed solid renders
/// identically to the BRep it came from *at the same segment count* (the
/// invariant the root-mesh cache already relies on), and its
/// `apply_transform` is a vertex walk rather than a surface rebuild.
#[cfg(feature = "raster")]
fn freeze(solid: &Solid, segments: u32) -> Solid {
    Solid::from_mesh(solid.to_mesh(segments))
}

/// Fold every vertex of `solid`, transformed by `t`, into `fb`.
#[cfg(feature = "raster")]
fn accumulate_verts(fb: &mut crate::FramingBuilder, solid: &Solid, t: &KTransform) {
    let mesh = solid.to_mesh(0); // mesh-backed: a clone, not a tessellation
    for c in mesh.vertices.as_chunks::<3>().0 {
        let p = t.apply_point(&vcad_kernel::vcad_kernel_math::Point3::new(
            c[0] as f64,
            c[1] as f64,
            c[2] as f64,
        ));
        fb.add([p.x, p.y, p.z]);
    }
}

/// Kernel rigid transform — the pose type the FK solver hands back.
type KTransform = vcad_kernel::vcad_kernel_math::Transform;

/// The eight corners of a solid's axis-aligned bounds, in its own frame.
///
/// A degenerate (empty) solid yields eight copies of the origin, which the
/// framing builder folds in harmlessly; `FramingBuilder::finish` is what
/// fails closed when *nothing* has extent.
#[cfg(feature = "raytrace")]
fn solid_bbox_corners(solid: &Solid) -> [[f64; 3]; 8] {
    let mesh = solid.to_mesh(crate::TESSELLATION_SEGMENTS);
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for c in mesh.vertices.as_chunks::<3>().0 {
        for i in 0..3 {
            lo[i] = lo[i].min(c[i] as f64);
            hi[i] = hi[i].max(c[i] as f64);
        }
    }
    if !lo.iter().all(|v| v.is_finite()) {
        return [[0.0; 3]; 8];
    }
    std::array::from_fn(|k| {
        [
            if k & 1 == 0 { lo[0] } else { hi[0] },
            if k & 2 == 0 { lo[1] } else { hi[1] },
            if k & 4 == 0 { lo[2] } else { hi[2] },
        ]
    })
}

/// Render a jointed assembly over a timeline in the **default drafting
/// line-art style** — the same projection, shading ramp, and hidden-line
/// overlay a single `-o frame.png` render produces, one PNG per frame.
///
/// Geometry is evaluated once and tessellated once; a frame costs only a
/// vertex transform per part plus the projection/rasterization, which is why
/// this tier runs in tens of milliseconds a frame where the path tracer
/// takes seconds. The camera is pinned across the sequence (see
/// [`crate::FixedFraming`]), so every PNG has identical dimensions and the
/// machine stays put while its joints move.
#[cfg(feature = "raster")]
pub fn render_drafting_animation(
    raw_vcad: &str,
    opts: &RasterOptions,
    an: &AnimateOptions,
    progress: &mut dyn FnMut(usize, usize, Duration),
) -> Result<AnimateReport, String> {
    check_animation_raster_opts(opts, "drafting")?;
    if opts.section.is_some() {
        return Err(
            "--section does not compose with --animate: the cut is a boolean \
             against tessellated geometry, which the animation path freezes \
             once and only re-poses. Drop --section."
                .to_string(),
        );
    }

    let evals_before = crate::document_eval_count();
    let AnimationPlan {
        statics,
        parts,
        poses,
        fps,
        eval,
    } = plan_animation(raw_vcad, an)?;

    let t_setup = Instant::now();
    // Tessellate at exactly the segment count the raster path would have
    // chosen for this canvas, so a frame is what a single still would have
    // drawn.
    let segments = crate::tessellation_segments(Some(opts.size_px));
    let static_solids: Vec<Solid> = statics.iter().map(|s| freeze(&s.solid, segments)).collect();
    let part_solids: Vec<Solid> = parts.iter().map(|s| freeze(&s.solid, segments)).collect();

    // Columns the raster path wants, statics first then instances — the same
    // order `evaluate_vcad` produces for a still.
    let tints: Vec<Option<[f64; 3]>> = statics.iter().chain(parts.iter()).map(|s| s.tint).collect();
    let names: Vec<Option<String>> = statics
        .iter()
        .chain(parts.iter())
        .map(|s| s.name.clone())
        .collect();

    // ── frame the camera on the union of every pose ──────────────────────
    let mut fb = crate::FramingBuilder::new(opts.view);
    let identity = KTransform::identity();
    for s in &static_solids {
        accumulate_verts(&mut fb, s, &identity);
    }
    for pose in &poses {
        for (s, t) in part_solids.iter().zip(pose) {
            accumulate_verts(&mut fb, s, t);
        }
    }
    let framing = fb.finish()?;
    let setup = t_setup.elapsed();

    std::fs::create_dir_all(&an.out_dir)
        .map_err(|e| format!("create {}: {e}", an.out_dir.display()))?;

    let mut report = AnimateReport {
        frames: Vec::with_capacity(poses.len()),
        fps,
        eval,
        setup,
        per_frame: Vec::with_capacity(poses.len()),
        parts: static_solids.len() + part_solids.len(),
        articulated: part_solids.len(),
    };
    let total = poses.len();
    for (i, pose) in poses.iter().enumerate() {
        let t_frame = Instant::now();
        let mut frame_solids: Vec<Solid> = static_solids.clone();
        frame_solids.extend(
            part_solids
                .iter()
                .zip(pose)
                .map(|(s, t)| s.apply_transform(t)),
        );
        let png =
            crate::raster::render_png_solids_framed(&frame_solids, &tints, &names, opts, &framing)?;
        let path = an.out_dir.join(format!("frame_{i:04}.png"));
        std::fs::write(&path, png).map_err(|e| format!("write {}: {e}", path.display()))?;
        let elapsed = t_frame.elapsed();
        report.frames.push(path);
        report.per_frame.push(elapsed);
        progress(i + 1, total, elapsed);
    }

    let evals = crate::document_eval_count() - evals_before;
    debug_assert_eq!(evals, 1, "geometry was evaluated {evals} times, not once");
    Ok(report)
}

/// Render a jointed assembly over a timeline with direct BRep ray tracing —
/// the `--raytrace` middle tier between drafting line art and the path
/// tracer.
///
/// Analytic surfaces mean the geometry cannot be frozen to triangles (that
/// is the whole point of the tier), so each frame re-places the BReps and
/// rebuilds their BVHs; evaluation still happens exactly once. Framing is
/// pinned the same way as the drafting tier — from the union of every posed
/// part's *bounds*, which is what the ray-traced still frames on too — so
/// frames come out uniformly sized.
#[cfg(feature = "raytrace")]
pub fn render_raytrace_animation(
    raw_vcad: &str,
    opts: &RasterOptions,
    an: &AnimateOptions,
    progress: &mut dyn FnMut(usize, usize, Duration),
) -> Result<AnimateReport, String> {
    check_animation_raster_opts(opts, "ray-traced")?;
    if opts.section.is_some() {
        return Err(
            "--section does not compose with --raytrace --animate; render the \
             drafting tier for a sectioned sequence"
                .to_string(),
        );
    }

    let evals_before = crate::document_eval_count();
    let AnimationPlan {
        statics,
        parts,
        poses,
        fps,
        eval,
    } = plan_animation(raw_vcad, an)?;

    let t_setup = Instant::now();
    // Framing from posed bounding boxes: the ray-traced still frames on BVH
    // root AABBs, so matching it here keeps the two paths reading alike.
    // Each part's *local* bbox corners are computed once (a tessellation
    // each, not one per frame) and then merely transformed per pose.
    let mut fb = crate::FramingBuilder::new(opts.view);
    for s in &statics {
        for c in solid_bbox_corners(&s.solid) {
            fb.add(c);
        }
    }
    let local_corners: Vec<[[f64; 3]; 8]> =
        parts.iter().map(|s| solid_bbox_corners(&s.solid)).collect();
    for pose in &poses {
        for (corners, t) in local_corners.iter().zip(pose) {
            for c in corners {
                let p = t.apply_point(&vcad_kernel::vcad_kernel_math::Point3::new(
                    c[0], c[1], c[2],
                ));
                fb.add([p.x, p.y, p.z]);
            }
        }
    }
    let framing = fb.finish()?;
    let setup = t_setup.elapsed();

    std::fs::create_dir_all(&an.out_dir)
        .map_err(|e| format!("create {}: {e}", an.out_dir.display()))?;

    let mut report = AnimateReport {
        frames: Vec::with_capacity(poses.len()),
        fps,
        eval,
        setup,
        per_frame: Vec::with_capacity(poses.len()),
        parts: statics.len() + parts.len(),
        articulated: parts.len(),
    };
    let total = poses.len();
    for (i, pose) in poses.iter().enumerate() {
        let t_frame = Instant::now();
        let mut scene: Vec<crate::SceneSolid> = statics.clone();
        scene.extend(parts.iter().zip(pose).map(|(s, t)| crate::SceneSolid {
            solid: s.solid.apply_transform(t),
            tint: s.tint,
            material: s.material.clone(),
            name: s.name.clone(),
            labels: s.labels.clone(),
            id: s.id.clone(),
        }));
        let png = crate::render_raytrace_png_solids_framed(&scene, opts, &framing)?;
        let path = an.out_dir.join(format!("frame_{i:04}.png"));
        std::fs::write(&path, png).map_err(|e| format!("write {}: {e}", path.display()))?;
        let elapsed = t_frame.elapsed();
        report.frames.push(path);
        report.per_frame.push(elapsed);
        progress(i + 1, total, elapsed);
    }

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

    /// A private, per-test output directory under the system temp dir.
    fn scratch_dir(tag: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vcad-render-anim-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    /// Decoded pixel dimensions of a written frame.
    fn png_size(path: &PathBuf) -> (u32, u32) {
        let img = image::open(path).expect("decode frame");
        (
            image::GenericImageView::width(&img),
            image::GenericImageView::height(&img),
        )
    }

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

    /// The drafting tier renders the whole sequence, and every frame comes
    /// out the same size — the property a per-frame fit (or `--trim`) would
    /// break, and the one an mp4 muxer refuses to work without.
    #[test]
    fn drafting_animation_frames_share_one_size() {
        let dir = scratch_dir("size");
        let an = AnimateOptions {
            out_dir: dir.clone(),
            timeline: Some(joint_timeline("swing", 0.0, 90.0)),
            frames: None,
            fps: None,
        };
        // Small canvas, no supersampling: this is about framing and frame
        // count, not image quality, and it runs on every build.
        let opts = RasterOptions {
            size_px: 128,
            auto_aspect: true,
            aa: Some(1),
            ..Default::default()
        };
        let report = render_drafting_animation(
            &jointed_doc("Revolute", [0.0, 0.0, 1.0]),
            &opts,
            &an,
            &mut |_, _, _| {},
        )
        .expect("drafting animation");

        assert_eq!(report.frames.len(), 3, "1 s at 2 fps is three samples");
        let sizes: Vec<(u32, u32)> = report.frames.iter().map(png_size).collect();
        assert!(
            sizes.windows(2).all(|w| w[0] == w[1]),
            "frames differ in size: {sizes:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// …and the arm actually moves: identical frames would satisfy the size
    /// check above while rendering a still life.
    #[test]
    fn drafting_animation_frames_differ() {
        let dir = scratch_dir("motion");
        let an = AnimateOptions {
            out_dir: dir.clone(),
            timeline: Some(joint_timeline("swing", 0.0, 90.0)),
            frames: None,
            fps: None,
        };
        let opts = RasterOptions {
            size_px: 128,
            aa: Some(1),
            ..Default::default()
        };
        let report = render_drafting_animation(
            &jointed_doc("Revolute", [0.0, 0.0, 1.0]),
            &opts,
            &an,
            &mut |_, _, _| {},
        )
        .expect("drafting animation");
        let bytes: Vec<Vec<u8>> = report
            .frames
            .iter()
            .map(|p| std::fs::read(p).expect("read frame"))
            .collect();
        assert_ne!(bytes[0], bytes[1], "frames 0 and 1 are identical");
        assert_ne!(bytes[1], bytes[2], "frames 1 and 2 are identical");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--trim` and animation are mutually exclusive by construction, and
    /// the refusal has to name the flag to drop.
    #[test]
    fn trim_is_refused_with_animation() {
        let an = AnimateOptions {
            out_dir: scratch_dir("never"),
            timeline: Some(joint_timeline("swing", 0.0, 90.0)),
            frames: None,
            fps: None,
        };
        let opts = RasterOptions {
            size_px: 128,
            trim_margin_px: Some(0),
            ..Default::default()
        };
        let err = render_drafting_animation(
            &jointed_doc("Revolute", [0.0, 0.0, 1.0]),
            &opts,
            &an,
            &mut |_, _, _| {},
        )
        .expect_err("should refuse");
        assert!(err.contains("--trim"), "unhelpful error: {err}");
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
