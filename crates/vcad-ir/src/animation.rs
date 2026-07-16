//! Animation timeline IR — the time axis for a vcad document.
//!
//! A [`Timeline`] is data on the document, diffable and parametric like
//! everything else. Tracks keyframe named parameters, joint states,
//! instance visibility, or a global explode factor; camera motion is
//! expressed as shot *intents* (turntable, orbit, focus) rather than raw
//! keyframes so agents author cinematography declaratively.

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
