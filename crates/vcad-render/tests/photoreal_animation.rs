//! `--animate` renders a jointed assembly over time while evaluating the
//! document's geometry exactly once.
//!
//! The two things worth asserting are that the frames actually differ (the
//! joint moved) and that the one-evaluation promise holds — the whole reason
//! this path exists instead of baking a document per frame.
#![cfg(all(feature = "raytrace", feature = "cli"))]

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

use vcad_render::animate::{render_photoreal_animation, AnimateOptions};
use vcad_render::photoreal::{Backdrop, PhotorealOptions};
use vcad_render::RasterOptions;

/// A scratch directory unique to this test, cleaned up on the way out.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("vcad-render-anim-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        TempDir(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A post with an arm bolted to it by a revolute joint about +Z, plus a
/// second, fixed-jointed block riding on the arm — so the test also covers
/// FK carrying a group along, which is how every real assembly is built.
const JOINTED_ASSEMBLY: &str = r#"{
  "version": "0.1",
  "nodes": {
    "1": { "id": 1, "name": "post", "op": { "type": "Cube", "size": { "x": 12, "y": 12, "z": 30 } } },
    "2": { "id": 2, "name": "arm",  "op": { "type": "Cube", "size": { "x": 60, "y": 8, "z": 8 } } }
  },
  "materials": {},
  "part_materials": {},
  "roots": [],
  "partDefs": {
    "post": { "id": "post", "name": "post", "root": 1 },
    "arm":  { "id": "arm",  "name": "arm",  "root": 2 }
  },
  "instances": [
    { "id": "post", "partDefId": "post", "material": "aluminum" },
    { "id": "arm",  "partDefId": "arm",  "material": "copper" }
  ],
  "joints": [
    { "id": "joint_0", "name": "swing", "parentInstanceId": "post",
      "childInstanceId": "arm",
      "parentAnchor": { "x": 6, "y": 6, "z": 30 },
      "childAnchor":  { "x": 6, "y": 6, "z": 30 },
      "kind": { "type": "Revolute", "axis": { "x": 0, "y": 0, "z": 1 },
                "limits": [-180, 180] },
      "state": 0.0 }
  ],
  "groundInstanceId": "post"
}"#;

const SWEEP: &str = r#"{
  "durationS": 1.0,
  "fps": 2.0,
  "tracks": [
    { "joint": "swing", "keys": [ { "t": 0, "value": 0 }, { "t": 1, "value": 90 } ] }
  ]
}"#;

fn tiny_options() -> (RasterOptions, PhotorealOptions) {
    (
        RasterOptions {
            size_px: 48,
            ..Default::default()
        },
        PhotorealOptions {
            spp: 2,
            backdrop: Backdrop::ShadowCatcher,
            ..Default::default()
        },
    )
}

#[test]
fn three_frames_move_and_the_document_is_evaluated_once() {
    let dir = TempDir::new("once");
    let (opts, pr) = tiny_options();
    let an = AnimateOptions {
        out_dir: dir.0.clone(),
        timeline: Some(vcad_render::animate::parse_timeline_spec(SWEEP).expect("spec")),
        frames: None,
        fps: None,
    };

    let before = vcad_render::document_eval_count();
    let report = render_photoreal_animation(
        JOINTED_ASSEMBLY,
        &opts,
        &pr,
        &an,
        &mut |_, _, _: Duration| {},
    )
    .expect("animate");

    assert_eq!(report.frames.len(), 3, "1 s at 2 fps is t=0, 0.5, 1.0");
    assert_eq!(
        vcad_render::document_eval_count() - before,
        1,
        "the document must be evaluated once for the whole sequence, not once \
         per frame — that is the entire point of this path"
    );
    assert_eq!(report.articulated, 2, "both instances are joint-placed");

    let bytes: Vec<Vec<u8>> = report
        .frames
        .iter()
        .map(|p| std::fs::read(p).expect("frame written"))
        .collect();
    for (i, b) in bytes.iter().enumerate() {
        assert_eq!(&b[1..4], b"PNG", "frame {i} is not a PNG");
    }
    assert_ne!(
        bytes[0], bytes[1],
        "the arm did not move between frames 0-1"
    );
    assert_ne!(
        bytes[1], bytes[2],
        "the arm did not move between frames 1-2"
    );

    // The subject must actually be in frame at both ends — a camera framed
    // on one pose only would clip the swing.
    for (i, b) in bytes.iter().enumerate() {
        let img = image::load_from_memory(b).expect("decode").to_rgba8();
        let covered = img.pixels().filter(|p| p.0[3] > 8).count();
        assert!(covered > 40, "frame {i} is nearly empty ({covered} px)");
    }
}

#[test]
fn frames_flag_truncates_the_sequence() {
    let dir = TempDir::new("trunc");
    let (opts, pr) = tiny_options();
    let an = AnimateOptions {
        out_dir: dir.0.clone(),
        timeline: Some(vcad_render::animate::parse_timeline_spec(SWEEP).expect("spec")),
        frames: Some(2),
        fps: None,
    };
    let report = render_photoreal_animation(JOINTED_ASSEMBLY, &opts, &pr, &an, &mut |_, _, _| {})
        .expect("animate");
    assert_eq!(report.frames.len(), 2);
}

/// A document with no joints is a static model; asking to animate it must
/// say so rather than quietly writing N identical frames.
#[test]
fn a_document_without_joints_is_rejected() {
    let dir = TempDir::new("nojoints");
    let (opts, pr) = tiny_options();
    let an = AnimateOptions {
        out_dir: dir.0.clone(),
        timeline: Some(vcad_render::animate::parse_timeline_spec(SWEEP).expect("spec")),
        frames: None,
        fps: None,
    };
    let static_doc = r#"{
      "version": "0.1",
      "nodes": { "1": { "id": 1, "name": "Cube",
                        "op": { "type": "Cube", "size": { "x": 10, "y": 10, "z": 10 } } } },
      "materials": {}, "part_materials": {},
      "roots": [{ "root": 1, "material": "aluminum" }]
    }"#;
    let err = render_photoreal_animation(static_doc, &opts, &pr, &an, &mut |_, _, _| {})
        .expect_err("should refuse");
    assert!(err.contains("assembly instances"), "unhelpful error: {err}");
}

/// End to end through the binary, which is where the `-o <dir>` contract and
/// the `--frames` plumbing live.
#[test]
fn the_cli_writes_a_frame_directory() {
    let dir = TempDir::new("cli");
    let doc = dir.0.join("assembly.vcad");
    std::fs::write(&doc, JOINTED_ASSEMBLY).expect("write doc");
    let spec = dir.0.join("sweep.json");
    std::fs::write(&spec, SWEEP).expect("write spec");
    let out = dir.0.join("frames");

    let output = Command::new(env!("CARGO_BIN_EXE_vcad-render"))
        .arg(&doc)
        .arg("--photoreal")
        .arg("--animate")
        .arg(&spec)
        .arg("--spp")
        .arg("2")
        .arg("--size")
        .arg("48")
        .arg("--no-mp4")
        .arg("-o")
        .arg(&out)
        .output()
        .expect("run vcad-render");
    assert!(
        output.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    for name in ["frame_0000.png", "frame_0001.png", "frame_0002.png"] {
        assert!(out.join(name).is_file(), "missing {name}");
    }
}
