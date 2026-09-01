//! `--section` cross-sections and `--assembly` / `--explode` renders.
//!
//! Both go through the binary, because both are command-line surfaces and the
//! wiring (arg parsing, required-unless, SVG-only guards) is as easy to get
//! wrong as the geometry.
#![cfg(feature = "cli")]

use std::path::PathBuf;
use std::process::Command;

struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> Self {
        let p = std::env::temp_dir().join(format!("vcad_render_secasm_{tag}"));
        std::fs::remove_dir_all(&p).ok();
        std::fs::create_dir_all(&p).unwrap();
        TempDir(p)
    }
    fn write(&self, name: &str, body: &str) -> PathBuf {
        let p = self.0.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn render(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_vcad-render"))
        .args(args)
        .output()
        .expect("run vcad-render")
}

fn svg_of(out: &std::process::Output) -> String {
    assert!(
        out.status.success(),
        "render failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout.clone()).expect("utf-8 svg")
}

/// A tube sectioned through its wall shows the cut as a hatched annulus
/// between two concentric circles — the inner bore and the outer wall.
#[test]
fn section_of_a_tube_hatches_the_annulus() {
    let dir = TempDir::new("tube");
    // Outer r10, bore r6, 40 tall. Primitives sit base-on-z=0, so z=20 is
    // the middle of the wall.
    let input = dir.write(
        "tube.loon",
        "[difference [cylinder 6.0 40.0] [cylinder 10.0 40.0]]",
    );
    let svg = svg_of(&render(&[
        input.to_str().unwrap(),
        "--view",
        "top",
        "--section",
        "z=20",
    ]));

    // The cut face is filled with the hatch pattern, and the pattern is
    // actually defined (a fill referencing a missing pattern renders blank).
    assert!(
        svg.contains(r#"id="section-hatch""#),
        "no hatch pattern defined"
    );
    assert!(
        svg.contains("url(#section-hatch)"),
        "hatch pattern defined but never used — nothing was detected as a cut face"
    );

    // Looking straight down the axis, the cut face is an ANNULUS: hatched
    // between the bore (r6) and the outer wall (r10), with the bore left
    // open. Measuring the hatched geometry's radial extent proves both
    // circles survived — a section that lost the bore would hatch a full
    // disc reaching r = 0, and one that lost the outer wall would stop short.
    let mut xs: Vec<(f64, f64)> = Vec::new();
    for chunk in svg.split("<polygon").skip(1) {
        let Some(tag_end) = chunk.find('>') else { continue };
        let tag = &chunk[..tag_end];
        if !tag.contains("url(#section-hatch)") {
            continue;
        }
        let Some(pts) = tag.split("points=\"").nth(1).and_then(|s| s.split('"').next()) else {
            continue;
        };
        for pair in pts.split_whitespace() {
            if let Some((x, y)) = pair.split_once(',') {
                if let (Ok(x), Ok(y)) = (x.parse::<f64>(), y.parse::<f64>()) {
                    xs.push((x, y));
                }
            }
        }
    }
    assert!(!xs.is_empty(), "no hatched geometry found");

    let (cx, cy) = (
        xs.iter().map(|p| p.0).sum::<f64>() / xs.len() as f64,
        xs.iter().map(|p| p.1).sum::<f64>() / xs.len() as f64,
    );
    let radii: Vec<f64> = xs.iter().map(|p| (p.0 - cx).hypot(p.1 - cy)).collect();
    let (rmin, rmax) = (
        radii.iter().cloned().fold(f64::MAX, f64::min),
        radii.iter().cloned().fold(f64::MIN, f64::max),
    );
    // Default scale is 2 px/mm, so the bore is ~12 px and the wall ~20 px.
    // The bands are generous because the circles are tessellated: an
    // inscribed polygon sits just inside its nominal radius.
    assert!(
        (10.5..13.0).contains(&rmin),
        "hatched inner radius {rmin:.2} px is not the r6 bore (~12 px) — \
         the bore was lost, or the whole disc got hatched"
    );
    assert!(
        (19.0..21.5).contains(&rmax),
        "hatched outer radius {rmax:.2} px is not the r10 wall (~20 px)"
    );
}

/// A section plane placed outside the model says so, instead of the old
/// generic "no solids produced" which reads as a broken document.
#[test]
fn section_outside_the_model_explains_itself() {
    let dir = TempDir::new("miss");
    let input = dir.write("cyl.loon", "[cylinder 10.0 40.0]");
    let out = render(&[
        input.to_str().unwrap(),
        "--view",
        "top",
        "--section",
        "z=0",
    ]);
    assert!(!out.status.success(), "a section that removes everything should fail");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("removed every part"),
        "unhelpful error: {err}"
    );
}

fn two_part_assembly(dir: &TempDir) -> PathBuf {
    dir.write("base.loon", "[cylinder 10.0 5.0]");
    dir.write("cap.loon", "[cylinder 8.0 4.0]");
    dir.write(
        "asm.json",
        r#"{
          "parts": [
            { "name": "base", "source": "base.loon" },
            { "name": "cap",  "source": "cap.loon" }
          ],
          "instances": [
            { "name": "base-1", "part": "base", "x": 0, "y": 0, "z": 0,
              "rx": 0, "ry": 0, "rz": 0, "ex": 0, "ey": 0, "ez": 0 },
            { "name": "cap-1",  "part": "cap",  "x": 0, "y": 0, "z": 5,
              "rx": 0, "ry": 0, "rz": 0, "ex": 0, "ey": 0, "ez": 50 }
          ]
        }"#,
    )
}

/// The exploded render must actually place the parts apart: the assembled
/// view is 9 mm tall (5 + 4), the exploded one adds `factor * 50`.
#[test]
fn exploded_assembly_places_parts_at_offset_positions() {
    let dir = TempDir::new("explode");
    let spec = two_part_assembly(&dir);

    let height = |factor: &str| -> f64 {
        let out = render(&[
            "--assembly",
            spec.to_str().unwrap(),
            "--explode",
            factor,
            "--view",
            "front",
        ]);
        let svg = svg_of(&out);
        svg.split("viewBox=\"")
            .nth(1)
            .and_then(|s| s.split('"').next())
            .expect("viewBox")
            .split_whitespace()
            .nth(3)
            .and_then(|h| h.parse::<f64>().ok())
            .expect("viewBox height")
    };

    let assembled = height("0");
    let exploded = height("0.6");
    // 0.6 * 50 = 30 mm of separation at 2 px/mm = ~60 px taller.
    assert!(
        exploded > assembled + 40.0,
        "explode did not separate the parts: assembled {assembled}, exploded {exploded}"
    );
    // The assembled view is the two parts stacked: ~9 mm tall, ~18 px.
    assert!(
        assembled < 40.0,
        "assembled view {assembled} is already exploded"
    );
}

/// Explode factor 0 is exactly the assembled view — the property that lets a
/// build sheet render both from one file.
#[test]
fn explode_zero_matches_the_assembled_pose() {
    let dir = TempDir::new("zero");
    let spec = two_part_assembly(&dir);
    let a = svg_of(&render(&[
        "--assembly",
        spec.to_str().unwrap(),
        "--view",
        "front",
    ]));
    let b = svg_of(&render(&[
        "--assembly",
        spec.to_str().unwrap(),
        "--explode",
        "0",
        "--view",
        "front",
    ]));
    assert_eq!(a, b, "default explode is not the assembled view");
}

/// Both parts reach the drawing. A silently-dropped instance would still
/// produce a plausible-looking SVG, so count the parts by focusing on each.
#[test]
fn every_instance_reaches_the_render() {
    let dir = TempDir::new("instances");
    let spec = two_part_assembly(&dir);
    for name in ["base-1", "cap-1"] {
        let out = render(&[
            "--assembly",
            spec.to_str().unwrap(),
            "--focus",
            name,
            "--view",
            "front",
        ]);
        assert!(
            out.status.success(),
            "--focus {name} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
}

#[test]
fn an_unknown_part_reference_is_an_error() {
    let dir = TempDir::new("badref");
    dir.write("base.loon", "[cylinder 10.0 5.0]");
    let spec = dir.write(
        "asm.json",
        r#"{"parts": [{"name": "base", "source": "base.loon"}],
            "instances": [{"name": "x", "part": "nope"}]}"#,
    );
    let out = render(&["--assembly", spec.to_str().unwrap()]);
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("undefined part 'nope'"), "{err}");
    // The error names what IS defined, so the fix is obvious.
    assert!(err.contains("base"), "{err}");
}

/// `--explode` without `--assembly` is a usage error rather than a silent
/// no-op, and an assembly render still honours `-o`.
#[test]
fn explode_requires_assembly_and_output_is_written() {
    let dir = TempDir::new("usage");
    let input = dir.write("cyl.loon", "[cylinder 10.0 5.0]");
    let out = render(&[input.to_str().unwrap(), "--explode", "0.5"]);
    assert!(!out.status.success(), "--explode alone should be rejected");

    let spec = two_part_assembly(&dir);
    let dest = dir.path("asm.svg");
    let out = render(&[
        "--assembly",
        spec.to_str().unwrap(),
        "-o",
        dest.to_str().unwrap(),
    ]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let svg = std::fs::read_to_string(&dest).unwrap();
    assert!(svg.contains("<svg"), "not an SVG: {svg:.80}");
}
