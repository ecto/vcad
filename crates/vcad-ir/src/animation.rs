//! Animation timeline IR — the time axis for a vcad document.
//!
//! A [`Timeline`] is data on the document, diffable and parametric like
//! everything else. Tracks keyframe named parameters, joint states,
//! instance visibility, or a global explode factor; camera motion is
//! expressed as shot *intents* (turntable, orbit, focus) rather than raw
//! keyframes so agents author cinematography declaratively.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Vec3;

/// Interpolation easing between two keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum Ease {
    /// Linear interpolation.
    #[default]
    Linear,
    /// Hold the previous key's value until this key's time (step function).
    Step,
    /// Smooth cubic ease-in-out (Hermite smoothstep).
    EaseInOut,
}

/// A single keyframe: value at time `t` (seconds), eased from the previous key.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct AnimKey {
    /// Time in seconds from the start of the timeline.
    pub t: f64,
    /// Target value at this key (units depend on the track target:
    /// parameter units, joint degrees/mm, visibility 0..1, explode factor).
    pub value: f64,
    /// Easing applied when interpolating from the previous key to this one.
    #[serde(default, skip_serializing_if = "is_default_ease")]
    pub ease: Ease,
}

fn is_default_ease(e: &Ease) -> bool {
    *e == Ease::Linear
}

/// What a track animates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum AnimTarget {
    /// A named document parameter (re-evaluates geometry per sample).
    Parameter {
        /// Parameter name as declared in `Document::parameters`.
        name: String,
    },
    /// A joint's driven state (degrees for revolute, mm for slider).
    Joint {
        /// Joint id in `Document::joints`.
        #[serde(rename = "jointId")]
        joint_id: String,
    },
    /// Instance visibility (value > 0.5 → visible).
    Visibility {
        /// Instance id in `Document::instances`.
        #[serde(rename = "instanceId")]
        instance_id: String,
    },
    /// Global exploded-view factor: 0 = assembled, 1 = fully exploded.
    /// Instances translate outward from the assembly centroid.
    Explode,
}

/// One animated channel: a target plus its keyframes (sorted by `t`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct AnimTrack {
    /// What this track animates.
    pub target: AnimTarget,
    /// Keyframes, ascending in time.
    pub keys: Vec<AnimKey>,
}

/// A camera shot intent spanning a time range on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct CameraShot {
    /// Shot start time in seconds.
    #[serde(rename = "startS")]
    pub start_s: f64,
    /// Shot end time in seconds.
    #[serde(rename = "endS")]
    pub end_s: f64,
    /// The shot intent.
    pub kind: CameraShotKind,
}

/// Declarative camera moves compiled to per-frame poses by the sequencer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub enum CameraShotKind {
    /// Full or partial turntable around the scene (or focus target).
    Turntable {
        /// Total sweep in degrees over the shot (360 = one revolution).
        degrees: f64,
        /// Camera elevation above the horizon in degrees.
        #[serde(rename = "elevationDeg", default = "default_elevation")]
        elevation_deg: f64,
    },
    /// Orbit between explicit start and end azimuth/elevation.
    Orbit {
        /// Start [azimuth, elevation] in degrees.
        from: [f64; 2],
        /// End [azimuth, elevation] in degrees.
        to: [f64; 2],
    },
    /// Hold a fixed view focused on a part or instance (dolly in slightly).
    Focus {
        /// Part or instance id to frame.
        target: String,
        /// Optional dolly factor over the shot (1 = none, 0.5 = halve distance).
        #[serde(default = "default_dolly", skip_serializing_if = "is_one")]
        dolly: f64,
    },
    /// Static isometric view (the default when no camera track exists).
    Static {
        /// Optional explicit eye direction; defaults to the standard isometric.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[cfg_attr(feature = "ts-rs", ts(optional))]
        direction: Option<Vec3>,
    },
}

fn default_elevation() -> f64 {
    30.0
}
fn default_dolly() -> f64 {
    1.0
}
fn is_one(v: &f64) -> bool {
    *v == 1.0
}

/// The document's time axis: duration, sampling rate, animated tracks, and
/// camera shots. Optional on the document; absence means a static model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct Timeline {
    /// Total duration in seconds.
    #[serde(rename = "durationS")]
    pub duration_s: f64,
    /// Sampling rate in frames per second (default 24).
    #[serde(default = "default_fps")]
    pub fps: f64,
    /// Animated tracks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tracks: Vec<AnimTrack>,
    /// Camera shots covering the timeline (gaps fall back to `Static`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub camera: Vec<CameraShot>,
}

fn default_fps() -> f64 {
    24.0
}

/// A per-frame camera pose in orbit coordinates, compiled from the
/// timeline's declarative [`CameraShot`]s by [`Timeline::sample_sequence`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct CameraPose {
    /// Azimuth around the scene, degrees.
    #[serde(rename = "azimuthDeg")]
    pub azimuth_deg: f64,
    /// Elevation above the horizon, degrees.
    #[serde(rename = "elevationDeg")]
    pub elevation_deg: f64,
    /// Distance factor (1 = default framing, 0.5 = half distance).
    pub dolly: f64,
    /// Optional part/instance id the camera is framing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "ts-rs", ts(optional))]
    pub target: Option<String>,
}

impl Default for CameraPose {
    fn default() -> Self {
        Self {
            azimuth_deg: 0.0,
            elevation_deg: 30.0,
            dolly: 1.0,
            target: None,
        }
    }
}

/// Sampled state for a single frame of the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ts-rs", derive(ts_rs::TS))]
#[cfg_attr(feature = "ts-rs", ts(export, export_to = "bindings/"))]
pub struct SequenceFrame {
    /// Frame index (0-based).
    pub index: u32,
    /// Time in seconds.
    pub t: f64,
    /// Animated parameter name → value at `t`.
    pub params: BTreeMap<String, f64>,
    /// Joint id → state at `t`.
    pub joints: BTreeMap<String, f64>,
    /// Instance id → visible (track value > 0.5).
    pub visibility: BTreeMap<String, bool>,
    /// Global exploded-view factor (0 = assembled).
    pub explode: f64,
    /// Camera pose compiled from the timeline's shots.
    pub camera: CameraPose,
    /// True iff params differ from the previous frame (frame 0: true if
    /// any parameter track exists).
    #[serde(rename = "geometryDirty")]
    pub geometry_dirty: bool,
}

impl Timeline {
    /// Number of frames when sampled at `fps` (at least 1, inclusive of t=0).
    pub fn frame_count(&self) -> usize {
        ((self.duration_s * self.fps).round() as usize).max(1) + 1
    }

    /// Sample a track's value at time `t` with easing between keys.
    pub fn sample_track(track: &AnimTrack, t: f64) -> Option<f64> {
        let keys = &track.keys;
        let first = keys.first()?;
        if t <= first.t {
            return Some(first.value);
        }
        let last = keys.last()?;
        if t >= last.t {
            return Some(last.value);
        }
        let idx = keys.iter().position(|k| k.t > t)?;
        let a = &keys[idx - 1];
        let b = &keys[idx];
        let span = b.t - a.t;
        let u = if span <= 0.0 { 1.0 } else { (t - a.t) / span };
        let u = match b.ease {
            Ease::Linear => u,
            Ease::Step => {
                if u >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
            Ease::EaseInOut => u * u * (3.0 - 2.0 * u),
        };
        Some(a.value + (b.value - a.value) * u)
    }

    /// Compile the camera shots into a pose at time `t`, carrying `prev`
    /// state. The last shot whose `[start_s, end_s)` contains `t` wins on
    /// overlap; gaps hold the previous pose.
    fn camera_pose_at(shots: &[CameraShot], t: f64, prev: &CameraPose) -> CameraPose {
        let active = shots.iter().rfind(|s| t >= s.start_s && t < s.end_s);
        let Some(shot) = active else {
            return prev.clone();
        };
        let span = shot.end_s - shot.start_s;
        let u = if span <= 0.0 {
            1.0
        } else {
            (t - shot.start_s) / span
        };
        match &shot.kind {
            CameraShotKind::Turntable {
                degrees,
                elevation_deg,
            } => CameraPose {
                azimuth_deg: degrees * u,
                elevation_deg: *elevation_deg,
                dolly: 1.0,
                target: None,
            },
            CameraShotKind::Orbit { from, to } => CameraPose {
                azimuth_deg: from[0] + (to[0] - from[0]) * u,
                elevation_deg: from[1] + (to[1] - from[1]) * u,
                dolly: 1.0,
                target: None,
            },
            CameraShotKind::Focus { target, dolly } => CameraPose {
                azimuth_deg: prev.azimuth_deg,
                elevation_deg: prev.elevation_deg,
                dolly: 1.0 + (dolly - 1.0) * u,
                target: Some(target.clone()),
            },
            CameraShotKind::Static { .. } => CameraPose::default(),
        }
    }

    /// Sample the timeline into a full per-frame sequence: track values
    /// bucketed by target kind, camera poses compiled from shot intents,
    /// and a `geometry_dirty` flag marking frames whose parameter values
    /// changed (frame 0 is dirty iff any parameter track exists).
    ///
    /// `fps <= 0` falls back to 24; the sequence always has at least 2
    /// frames (t=0 and the final frame) and is inclusive of both ends.
    pub fn sample_sequence(&self) -> Vec<SequenceFrame> {
        let fps = if self.fps > 0.0 { self.fps } else { 24.0 };
        let frame_count = (((self.duration_s * fps).round() as i64) + 1).max(2) as u32;
        let has_param_tracks = self
            .tracks
            .iter()
            .any(|tr| matches!(tr.target, AnimTarget::Parameter { .. }));

        let mut frames = Vec::with_capacity(frame_count as usize);
        let mut prev_params: Option<BTreeMap<String, f64>> = None;
        let mut prev_pose = CameraPose::default();

        for index in 0..frame_count {
            let t = f64::from(index) / fps;
            let mut params = BTreeMap::new();
            let mut joints = BTreeMap::new();
            let mut visibility = BTreeMap::new();
            let mut explode = 0.0;

            for track in &self.tracks {
                let value = Self::sample_track(track, t).unwrap_or(0.0);
                match &track.target {
                    AnimTarget::Parameter { name } => {
                        params.insert(name.clone(), value);
                    }
                    AnimTarget::Joint { joint_id } => {
                        joints.insert(joint_id.clone(), value);
                    }
                    AnimTarget::Visibility { instance_id } => {
                        visibility.insert(instance_id.clone(), value > 0.5);
                    }
                    AnimTarget::Explode => explode = value,
                }
            }

            let camera = Self::camera_pose_at(&self.camera, t, &prev_pose);
            prev_pose = camera.clone();

            let geometry_dirty = match &prev_params {
                None => has_param_tracks,
                Some(prev) => params.iter().any(|(name, v)| prev.get(name) != Some(v)),
            };
            prev_params = Some(params.clone());

            frames.push(SequenceFrame {
                index,
                t,
                params,
                joints,
                visibility,
                explode,
                camera,
                geometry_dirty,
            });
        }
        frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(keys: Vec<AnimKey>) -> AnimTrack {
        AnimTrack {
            target: AnimTarget::Explode,
            keys,
        }
    }

    fn key(t: f64, value: f64, ease: Ease) -> AnimKey {
        AnimKey { t, value, ease }
    }

    #[test]
    fn sample_linear_and_clamped() {
        let tr = track(vec![
            key(0.0, 0.0, Ease::Linear),
            key(2.0, 10.0, Ease::Linear),
        ]);
        assert_eq!(Timeline::sample_track(&tr, -1.0), Some(0.0));
        assert_eq!(Timeline::sample_track(&tr, 1.0), Some(5.0));
        assert_eq!(Timeline::sample_track(&tr, 3.0), Some(10.0));
    }

    #[test]
    fn sample_step_holds_previous() {
        let tr = track(vec![key(0.0, 1.0, Ease::Linear), key(1.0, 2.0, Ease::Step)]);
        assert_eq!(Timeline::sample_track(&tr, 0.5), Some(1.0));
        assert_eq!(Timeline::sample_track(&tr, 1.0), Some(2.0));
    }

    #[test]
    fn sample_ease_in_out_midpoint() {
        let tr = track(vec![
            key(0.0, 0.0, Ease::Linear),
            key(1.0, 1.0, Ease::EaseInOut),
        ]);
        assert_eq!(Timeline::sample_track(&tr, 0.5), Some(0.5));
        let quarter = Timeline::sample_track(&tr, 0.25).unwrap();
        assert!(quarter < 0.25, "ease-in should lag linear early on");
    }

    #[test]
    fn frame_count_inclusive() {
        let tl = Timeline {
            duration_s: 2.0,
            fps: 24.0,
            tracks: vec![],
            camera: vec![],
        };
        assert_eq!(tl.frame_count(), 49);
    }

    fn timeline(
        duration_s: f64,
        fps: f64,
        tracks: Vec<AnimTrack>,
        camera: Vec<CameraShot>,
    ) -> Timeline {
        Timeline {
            duration_s,
            fps,
            tracks,
            camera,
        }
    }

    #[test]
    fn sequence_frame_count_and_fps_fallback() {
        let frames = timeline(2.0, 24.0, vec![], vec![]).sample_sequence();
        assert_eq!(frames.len(), 49);
        assert_eq!(frames[0].t, 0.0);
        assert!((frames[48].t - 2.0).abs() < 1e-12);
        // fps <= 0 falls back to 24
        assert_eq!(
            timeline(1.0, 0.0, vec![], vec![]).sample_sequence().len(),
            25
        );
        // at least 2 frames
        assert_eq!(
            timeline(0.0, 24.0, vec![], vec![]).sample_sequence().len(),
            2
        );
    }

    #[test]
    fn sequence_tracks_and_geometry_dirty() {
        let tl = timeline(
            1.0,
            4.0,
            vec![
                AnimTrack {
                    target: AnimTarget::Parameter {
                        name: "width".into(),
                    },
                    keys: vec![key(0.0, 10.0, Ease::Linear), key(1.0, 20.0, Ease::Linear)],
                },
                AnimTrack {
                    target: AnimTarget::Joint {
                        joint_id: "j1".into(),
                    },
                    keys: vec![key(0.0, 0.0, Ease::Linear), key(1.0, 90.0, Ease::Linear)],
                },
                AnimTrack {
                    target: AnimTarget::Visibility {
                        instance_id: "lid".into(),
                    },
                    keys: vec![key(0.0, 1.0, Ease::Linear), key(0.5, 0.0, Ease::Step)],
                },
                AnimTrack {
                    target: AnimTarget::Explode,
                    keys: vec![key(0.0, 0.0, Ease::Linear), key(1.0, 1.0, Ease::Linear)],
                },
            ],
            vec![],
        );
        let frames = tl.sample_sequence();
        assert_eq!(frames.len(), 5);
        assert_eq!(frames[0].params["width"], 10.0);
        assert_eq!(frames[2].params["width"], 15.0);
        assert_eq!(frames[4].joints["j1"], 90.0);
        assert!(frames[0].visibility["lid"]);
        assert!(!frames[3].visibility["lid"]);
        assert_eq!(frames[2].explode, 0.5);
        assert!(frames.iter().all(|f| f.geometry_dirty));
        // static params → only frame 0 dirty; no param tracks → never dirty
        let static_tl = timeline(
            1.0,
            2.0,
            vec![AnimTrack {
                target: AnimTarget::Parameter {
                    name: "width".into(),
                },
                keys: vec![key(0.0, 10.0, Ease::Linear)],
            }],
            vec![],
        );
        let sf = static_tl.sample_sequence();
        assert!(sf[0].geometry_dirty);
        assert!(!sf[1].geometry_dirty);
        let none = timeline(1.0, 2.0, vec![], vec![]).sample_sequence();
        assert!(!none[0].geometry_dirty);
    }

    #[test]
    fn sequence_camera_shots() {
        // Turntable sweeps azimuth; t past the shot holds the last pose.
        let frames = timeline(
            2.0,
            2.0,
            vec![],
            vec![CameraShot {
                start_s: 0.0,
                end_s: 2.0,
                kind: CameraShotKind::Turntable {
                    degrees: 360.0,
                    elevation_deg: 45.0,
                },
            }],
        )
        .sample_sequence();
        assert_eq!(frames[0].camera.azimuth_deg, 0.0);
        assert_eq!(frames[0].camera.elevation_deg, 45.0);
        assert!((frames[2].camera.azimuth_deg - 180.0).abs() < 1e-12);
        assert_eq!(frames[4].camera.azimuth_deg, frames[3].camera.azimuth_deg);

        // No shots → default pose.
        let plain = timeline(1.0, 2.0, vec![], vec![]).sample_sequence();
        assert_eq!(plain[0].camera, CameraPose::default());

        // Orbit then Focus: focus holds prior az/el, dollies toward target.
        let frames = timeline(
            2.0,
            2.0,
            vec![],
            vec![
                CameraShot {
                    start_s: 0.0,
                    end_s: 1.0,
                    kind: CameraShotKind::Orbit {
                        from: [0.0, 10.0],
                        to: [90.0, 40.0],
                    },
                },
                CameraShot {
                    start_s: 1.0,
                    end_s: 2.0,
                    kind: CameraShotKind::Focus {
                        target: "lid".into(),
                        dolly: 0.5,
                    },
                },
            ],
        )
        .sample_sequence();
        assert!((frames[1].camera.azimuth_deg - 45.0).abs() < 1e-12);
        assert!((frames[3].camera.dolly - 0.75).abs() < 1e-12);
        assert_eq!(frames[3].camera.target.as_deref(), Some("lid"));
        assert_eq!(frames[3].camera.azimuth_deg, frames[1].camera.azimuth_deg);
    }

    #[test]
    fn sequence_frame_json_shape() {
        let frames = timeline(0.0, 24.0, vec![], vec![]).sample_sequence();
        let json = serde_json::to_string(&frames[0]).unwrap();
        assert!(json.contains(r#""geometryDirty""#));
        assert!(json.contains(r#""azimuthDeg""#));
        // absent target is omitted, matching the TS CameraPose shape
        assert!(!json.contains(r#""target""#));
    }

    #[test]
    fn timeline_json_roundtrip() {
        let tl = Timeline {
            duration_s: 3.0,
            fps: 24.0,
            tracks: vec![AnimTrack {
                target: AnimTarget::Joint {
                    joint_id: "j1".into(),
                },
                keys: vec![
                    key(0.0, 0.0, Ease::Linear),
                    key(3.0, 360.0, Ease::EaseInOut),
                ],
            }],
            camera: vec![CameraShot {
                start_s: 0.0,
                end_s: 3.0,
                kind: CameraShotKind::Turntable {
                    degrees: 360.0,
                    elevation_deg: 30.0,
                },
            }],
        };
        let json = serde_json::to_string(&tl).unwrap();
        assert!(json.contains(r#""durationS""#));
        assert!(json.contains(r#""jointId""#));
        let back: Timeline = serde_json::from_str(&json).unwrap();
        assert_eq!(back, tl);
    }
}
