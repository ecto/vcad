//! `vcad cam` end to end: the binary, the checked-in fixtures, the exit codes.
//!
//! These run the real command, because the thing worth testing here is not
//! that [`vcad_cam_api`] computes the right numbers — it has its own tests for
//! that — but that the *command* carries them across correctly and, above all,
//! that **a blocked job leaves no file on disk**. A unit test cannot observe
//! that; only a process that ran and a directory that is still empty can.
//!
//! Every assertion is a number or a file, never "it did something".

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

/// The binary under test, built by cargo for this integration target.
const VCAD: &str = env!("CARGO_BIN_EXE_vcad");

fn repo() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/vcad-cli.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the crate sits two levels under the repo root")
        .to_path_buf()
}

fn fixture(name: &str) -> PathBuf {
    repo().join("docs/cam-fixtures").join(name)
}

/// Run `vcad --no-cache cam …`. The cache is off so `outline` always reaches
/// the B-rep: a root-mesh cache hit hands back triangles with no topology, and
/// the section would then quietly come off the export mesh.
fn cam(args: &[&str]) -> Output {
    Command::new(VCAD)
        .arg("--no-cache")
        .arg("cam")
        .args(args)
        .env("VCAD_CACHE", "0")
        .output()
        .expect("vcad should run")
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the process exited normally")
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn answer(path: &Path) -> Value {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{} was not written: {e}", path.display()));
    serde_json::from_str(&text).expect("the answer is JSON")
}

fn write(dir: &Path, name: &str, value: &Value) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_string(value).expect("serialisable")).expect("writable");
    path
}

// ---------------------------------------------------------------------------
// gear
// ---------------------------------------------------------------------------

/// The measurement the milestone is closed against, to the micron.
#[test]
fn gear_reports_the_planet_over_pins_and_span() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("gear.json");
    let run = cam(&[
        "gear",
        "--request",
        fixture("planet-gear.json").to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));

    let a = answer(&out);
    let report = &a["report"];
    let pins = &report["over_pins"];
    assert_eq!(pins["pin_diameter"].as_f64(), Some(1.4));
    let m = pins["dimension"].as_f64().expect("a dimension");
    assert!(
        (m - 21.0738).abs() < 1e-3,
        "over Ø1.4 pins the 20T planet measures M = {m}, not 21.0738"
    );
    let span = &report["span"];
    assert_eq!(span["teeth_spanned"].as_u64(), Some(3));
    let w = span["length"].as_f64().expect("a length");
    assert!(
        (w - 7.6322).abs() < 1e-3,
        "the base tangent over 3 teeth is W = {w}, not 7.6322"
    );

    // The sensitivity is what turns a measured M into a thickness error, so it
    // has to be on the report a shop reads.
    let sensitivity = pins["sensitivity"].as_f64().expect("a sensitivity");
    assert!(
        (sensitivity - 3.1776).abs() < 1e-3,
        "dM/dt at the pitch circle is {sensitivity}, not 3.1776"
    );

    // Every member of the train is reachable with the Ø1 cutter, and the
    // externals are exactly involute across contact.
    let reports = a["planetary"]["reports"].as_array().expect("three reports");
    assert_eq!(reports.len(), 3);
    for r in reports {
        let reach = &r["reachability"];
        assert_eq!(
            reach["ok"].as_bool(),
            Some(true),
            "{}T is not reachable at Ø1: {reach}",
            r["teeth"]
        );
    }
    // Sun, planet: clear by 67 µm and 178 µm of radius. Ring: by 0.19 µm.
    let margins: Vec<f64> = reports
        .iter()
        .map(|r| r["reachability"]["margin"].as_f64().expect("a margin"))
        .collect();
    assert!(
        (margins[0] - 0.067466).abs() < 1e-5,
        "sun margin {}",
        margins[0]
    );
    assert!(
        (margins[1] - 0.178061).abs() < 1e-5,
        "planet margin {}",
        margins[1]
    );
    assert!(
        (margins[2] - 0.000189).abs() < 1e-5,
        "the ring clears by {} mm, not 0.000189 — this is the tightest number in the train",
        margins[2]
    );
}

/// The ring without its backlash thinning is the case the roadmap recorded:
/// the fillet crosses the contact limit, and the verdict flips on the grading.
/// Strict is refused (exit 2); graded at 5 µm it passes, with the 1.36 µm of
/// metal that decides it reported either way.
#[test]
fn the_ring_verdict_flips_on_the_flank_tolerance() {
    let dir = tempfile::tempdir().expect("tempdir");
    let base = serde_json::json!({
        "gear": { "module": 1.0, "teeth": 50, "internal": true, "profile_shift": 0.47, "face_width": 6.0 },
        "cutter_diameter": 1.0,
        "contours": false,
        "planetary": {
            "sun":    { "module": 1.0, "teeth": 10, "profile_shift": 0.47, "tip_radius": 6.45,  "face_width": 5.6 },
            "planet": { "module": 1.0, "teeth": 20, "profile_shift": 0.0,  "tip_radius": 10.89, "face_width": 5.0 },
            "ring":   { "module": 1.0, "teeth": 50, "internal": true, "profile_shift": 0.47, "face_width": 6.0 },
            "planets": 3
        }
    });

    let strict_out = dir.path().join("strict.json");
    let strict = cam(&[
        "gear",
        "--request",
        write(dir.path(), "strict-request.json", &base)
            .to_str()
            .expect("utf8"),
        "--out",
        strict_out.to_str().expect("utf8"),
    ]);
    assert_eq!(
        code(&strict),
        2,
        "an unreachable flank is the answer being no, not a request that could not be read"
    );
    assert!(
        stderr(&strict).contains("cannot cut every flank"),
        "the refusal has to say what is wrong: {}",
        stderr(&strict)
    );
    let ring = &answer(&strict_out)["planetary"]["reports"][2];
    assert_eq!(ring["reachability"]["ok"].as_bool(), Some(false));
    assert_eq!(
        ring["reachability"]["encroachment"].as_str(),
        Some("Exceeds")
    );
    let deviation = ring["flank_deviation_at_contact_limit"]
        .as_f64()
        .expect("a deviation");
    assert!(
        (deviation - 0.0013557).abs() < 1e-6,
        "the ring's fillet stands {deviation} mm proud of the involute, not 0.0013557"
    );

    let mut graded = base.clone();
    graded["flank_tolerance"] = serde_json::json!(0.005);
    let graded_out = dir.path().join("graded.json");
    let run = cam(&[
        "gear",
        "--request",
        write(dir.path(), "graded-request.json", &graded)
            .to_str()
            .expect("utf8"),
        "--out",
        graded_out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));
    let ring = &answer(&graded_out)["planetary"]["reports"][2];
    assert_eq!(ring["reachability"]["ok"].as_bool(), Some(true));
    assert_eq!(
        ring["reachability"]["encroachment"].as_str(),
        Some("WithinTolerance")
    );
    // The geometry did not move: only the question changed.
    assert!(
        (ring["flank_deviation_at_contact_limit"]
            .as_f64()
            .expect("a deviation")
            - deviation)
            .abs()
            < 1e-12,
        "grading must not change the measurement it grades"
    );
}

// ---------------------------------------------------------------------------
// job + verify
// ---------------------------------------------------------------------------

/// The planet blank posts, writes its program, and that program verifies when
/// it is read straight back off disk.
#[test]
fn the_planet_blank_posts_and_its_own_program_verifies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let gcode = dir.path().join("planet-blank.nc");
    let out = dir.path().join("job.json");
    let run = cam(&[
        "job",
        "--request",
        fixture("planet-blank-job.json").to_str().expect("utf8"),
        "--gcode",
        gcode.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));

    let a = answer(&out);
    assert_eq!(a["blocked"].as_bool(), Some(false));
    assert!(
        gcode.is_file(),
        "a job that passed has to leave its program"
    );
    let program = std::fs::read_to_string(&gcode).expect("readable");
    assert!(
        program.lines().count() > 500,
        "a three-operation job through 5 mm of brass is more than {} lines",
        program.lines().count()
    );

    // Every error-severity check, by name, with the number it turned on.
    let v = &a["verification"];
    for (name, path) in [
        ("gouge", &v["gouge"]),
        ("material_left", &v["material_left"]["check"]),
        ("rapids", &v["rapids"]),
        ("depth", &v["depth"]["check"]),
        ("tabs", &v["tabs"]["check"]),
        ("envelope", &v["envelope"]["check"]),
    ] {
        assert_eq!(
            path["pass"].as_bool(),
            Some(true),
            "{name} failed: {} violation(s), worst {}",
            path["violation_count"],
            path["worst"]
        );
    }
    // The three tabs are cut, and they are 2 mm of metal each rather than
    // whatever the audit happened to notice.
    assert_eq!(v["tabs"]["tab_count"].as_u64(), Some(3));
    let tabs = a["tab_placement"][0]["tabs"]
        .as_array()
        .expect("three tabs");
    assert_eq!(tabs.len(), 3);
    for t in tabs {
        let metal = t["metal_width"].as_f64().expect("a width");
        let height = t["height"].as_f64().expect("a height");
        assert!(
            (metal - 2.0).abs() < 0.01,
            "a declared 2 mm tab came out {metal} mm wide"
        );
        // 0.5 mm of tab above a floor that is 0.2 mm into the spoilboard.
        assert!(
            (height - 0.7).abs() < 0.01,
            "a 0.5 mm tab over a 0.2 mm break-through stands {height} mm"
        );
    }

    // …and the file, replayed from disk against the same part, agrees.
    let verify_out = dir.path().join("verify.json");
    let run = cam(&[
        "verify",
        "--request",
        fixture("planet-blank-verify.json").to_str().expect("utf8"),
        "--gcode",
        gcode.to_str().expect("utf8"),
        "--out",
        verify_out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));
    let a = answer(&verify_out);
    assert_eq!(a["pass"].as_bool(), Some(true));
    assert_eq!(a["blocked"].as_bool(), Some(false));
    assert!(
        a["verification"]["moves"].as_u64().unwrap_or(0) > 1000,
        "the replay has to have walked the whole program"
    );
}

/// The rule the whole surface is built on: take the sacrificial board away
/// from a job that cuts past the underside and **no program is written**.
///
/// This refusal comes back before any toolpath exists, so `cam::job` returns
/// without reaching its writer at all — the file-absence assertion here is a
/// guard, not a measurement. The mutation that proves the writer is on
/// `a_job_the_oracle_blocks_writes_no_program` below, where a program does
/// exist and is withheld. What this test measures is the exit code and the
/// wording: the depth, and what to put under the stock.
#[test]
fn a_break_through_with_no_spoilboard_writes_no_program() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut job: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("planet-blank-job.json")).expect("the fixture"),
    )
    .expect("the fixture is JSON");

    // The same job, minus the board under it. Every operation still asks to
    // cut 0.2 mm past the underside.
    job["stock"]
        .as_object_mut()
        .expect("an object")
        .remove("spoilboard");
    for op in job["operations"].as_array().expect("operations") {
        assert_eq!(
            op["bottom_allowance"].as_f64(),
            Some(-0.2),
            "this test is only meaningful while the job breaks through"
        );
    }

    let request = write(dir.path(), "no-spoilboard.json", &job);
    let gcode = dir.path().join("must-not-exist.nc");
    let out = dir.path().join("job.json");
    let run = cam(&[
        "job",
        "--request",
        request.to_str().expect("utf8"),
        "--gcode",
        gcode.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);

    assert_eq!(
        code(&run),
        2,
        "a refused job exits 2, not 0 and not 1: stderr {}",
        stderr(&run)
    );
    assert!(
        !gcode.is_file(),
        "a refused job must leave NOTHING to send: {} exists",
        gcode.display()
    );

    let message = stderr(&run);
    assert!(
        message.contains("0.200 mm past the underside"),
        "the refusal has to name the depth: {message}"
    );
    assert!(
        message.contains("spoilboard"),
        "…and what to do about it: {message}"
    );
    // The answer document is still written: a refusal is the answer worth
    // keeping, it is the *program* that is withheld.
    assert!(answer(&out)["error"].is_string());
}

/// The other half of the same rule: a job the *oracle* blocks — the request
/// was buildable, the toolpath exists, and a check said no — also leaves
/// nothing behind. Take the tabs off the planet blank and the profile frees
/// the part with nothing holding it.
///
/// Mutation check — replace the `if !blocked` guard in `cam::job` with an
/// unconditional write of `answer["gcode"].unwrap_or("")` and this test fails
/// on `!gcode.is_file()`, having written a zero-byte program that a sender
/// would open without complaint.
#[test]
fn a_job_the_oracle_blocks_writes_no_program() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut job: Value = serde_json::from_str(
        &std::fs::read_to_string(fixture("planet-blank-job.json")).expect("the fixture"),
    )
    .expect("the fixture is JSON");

    for op in job["operations"].as_array_mut().expect("operations") {
        let op = op.as_object_mut().expect("an object");
        op.remove("tabs");
        op.remove("tab_width");
        op.remove("tab_height");
    }

    let request = write(dir.path(), "no-tabs.json", &job);
    let gcode = dir.path().join("must-not-exist.nc");
    let out = dir.path().join("job.json");
    let run = cam(&[
        "job",
        "--request",
        request.to_str().expect("utf8"),
        "--gcode",
        gcode.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);

    assert_eq!(code(&run), 2, "stderr: {}", stderr(&run));
    assert!(
        !gcode.is_file(),
        "a blocked job must leave NOTHING to send: {} exists",
        gcode.display()
    );

    let a = answer(&out);
    assert_eq!(a["blocked"].as_bool(), Some(true));
    assert!(
        a["gcode"].is_null(),
        "the answer document must not carry the program either"
    );
    assert_eq!(
        a["policy"]["blocked_by"]
            .as_array()
            .expect("named checks")
            .iter()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>(),
        vec!["loose_pieces"],
    );
    // The whole Ø21.78 disc less its bore — π·10.89² − π·1.9² = 361.3 mm² of
    // part, not a sliver of waste. The number is the point: `loose_pieces` is
    // a warning when waste comes free and an error when the part does.
    let freed = a["verification"]["loose"]["check"]["worst"]
        .as_f64()
        .expect("an area");
    assert!(
        (freed - 361.0).abs() < 1.0,
        "the whole blank comes free ({freed} mm²), which is why it is an error and not a warning"
    );
    assert!(
        stderr(&run).contains("loose_pieces"),
        "the refusal names the check: {}",
        stderr(&run)
    );
}

// ---------------------------------------------------------------------------
// outline
// ---------------------------------------------------------------------------

/// A solid in, a contour out: the plate's Ø16 bore comes back as a circle, and
/// the part is recognised as constant-section.
#[test]
fn outline_sections_the_parametric_plate_and_finds_its_bore() {
    let dir = tempfile::tempdir().expect("tempdir");
    let out = dir.path().join("outline.json");
    let plate = repo().join("examples/parametric-plate.vcad");
    let run = cam(&[
        "outline",
        plate.to_str().expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));

    let a = answer(&out);
    // The raw tessellation of the B-rep, not the repaired export mesh.
    assert_eq!(
        a["mesh_source"].as_str(),
        Some("raw_tessellation"),
        "a CAM contour has to come off the unrepaired tessellation"
    );
    // 80 x 50 x 6 plate: sectioned at its mid-height, 6 mm of stock.
    let thickness = a["suggested_stock_thickness"]
        .as_f64()
        .expect("a thickness");
    assert!(
        (thickness - 6.0).abs() < 1e-6,
        "the plate is {thickness} mm thick, not 6"
    );
    assert!((a["z"].as_f64().expect("a z") - 3.0).abs() < 1e-6);

    let region = &a["regions"][0];
    let area = region["area"].as_f64().expect("an area");
    // 80 x 50, less a Ø16 bore (200.74 mm² as a 64-gon) and four R1.5 corner
    // fillets (1.93 mm²).
    assert!(
        (area - 3797.03).abs() < 0.5,
        "the sectioned plate is {area} mm², not 3797.03"
    );
    assert_eq!(region["holes"].as_array().map(Vec::len), Some(1));
    let bounds = a["bounds"].as_array().expect("bounds");
    let b: Vec<f64> = bounds
        .iter()
        .map(|v| v.as_f64().expect("a number"))
        .collect();
    assert_eq!(
        b,
        vec![0.0, 0.0, 80.0, 50.0],
        "the blank the section sits in"
    );

    let circles = a["circles"].as_array().expect("circles");
    assert_eq!(circles.len(), 1, "one bore, not {}", circles.len());
    let d = circles[0]["diameter"].as_f64().expect("a diameter");
    assert!(
        (d - 16.0).abs() < 0.02,
        "the bore reads Ø{d}, not Ø16 — that is the number a cutter is sized against"
    );
    let centre = &circles[0]["center"];
    assert!((centre[0].as_f64().expect("x") - 40.0).abs() < 0.01);
    assert!((centre[1].as_f64().expect("y") - 25.0).abs() < 0.01);

    // And the part is NOT constant-section, which is the useful answer: the
    // example plate carries an R1.5 fillet on its edges, so a single contour
    // cut at one Z is 0.60 mm wrong at the top. A 2.5D job wants to be told
    // that before it cuts, not after.
    let p = &a["prismatic"];
    assert_eq!(p["prismatic"].as_bool(), Some(false), "{p}");
    let departure = p["max_boundary_distance"].as_f64().expect("a distance");
    assert!(
        (departure - 0.603).abs() < 0.01,
        "the R1.5 edge fillet puts the section {departure} mm off at its worst, not 0.603"
    );
    assert!(
        (p["worst_z"].as_f64().expect("a z") - 5.7).abs() < 1e-9,
        "and it is worst near the top face, at z {}",
        p["worst_z"]
    );
}

/// A torn solid is refused with its gaps rather than healed into something
/// that looks machinable. Item 30's signal, kept.
#[test]
fn outline_refuses_a_torn_solid_and_says_where() {
    let dir = tempfile::tempdir().expect("tempdir");
    let stl = dir.path().join("torn.stl");
    std::fs::write(&stl, torn_cube()).expect("writable");

    let out = dir.path().join("outline.json");
    let run = cam(&[
        "outline",
        stl.to_str().expect("utf8"),
        "--z",
        "5",
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 2, "a torn section is the answer being no");

    let message = stderr(&run);
    assert!(
        message.contains("does not close"),
        "the refusal has to say the section is open: {message}"
    );
    let a = answer(&out);
    let gaps = a["gaps"].as_array().expect("the gaps, not just a sentence");
    assert!(!gaps.is_empty(), "a refusal with no gaps is unactionable");
    let widest = gaps
        .iter()
        .filter_map(|g| g["distance"].as_f64())
        .fold(0.0f64, f64::max);
    assert!(
        (widest - 5.0).abs() < 1e-3,
        "the missing triangle opens a 5 mm slot in that wall at z 5, and the gap reads {widest}"
    );
    assert!(
        a["error"].as_str().expect("a sentence").contains("torn"),
        "and it has to say the solid is torn, not that the outline is: {}",
        a["error"]
    );
}

/// A 10 mm cube with one triangle of one wall deleted. Sectioned anywhere
/// through that wall the loop cannot close.
fn torn_cube() -> Vec<u8> {
    let v = [
        [0.0f32, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [10.0, 10.0, 0.0],
        [0.0, 10.0, 0.0],
        [0.0, 0.0, 10.0],
        [10.0, 0.0, 10.0],
        [10.0, 10.0, 10.0],
        [0.0, 10.0, 10.0],
    ];
    let faces: Vec<[usize; 3]> = vec![
        [0, 2, 1],
        [0, 3, 2], // bottom
        [4, 5, 6],
        [4, 6, 7], // top
        [0, 1, 5],
        [0, 5, 4], // y = 0
        [1, 2, 6],
        [1, 6, 5], // x = 10
        [2, 3, 7],
        [2, 7, 6], // y = 10
        // x = 0 would be [3, 0, 4] and [3, 4, 7]; the first is left out, which
        // opens a 10 mm slot in that wall at every height.
        [3, 4, 7],
    ];
    let mut out = vec![0u8; 84];
    out[80..84].copy_from_slice(&(faces.len() as u32).to_le_bytes());
    for f in faces {
        out.extend_from_slice(&[0u8; 12]); // normal, recomputed by every reader
        for i in f {
            for coordinate in v[i] {
                out.extend_from_slice(&coordinate.to_le_bytes());
            }
        }
        out.extend_from_slice(&[0u8; 2]);
    }
    out
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

/// The check that stops a stale DXF machining the wrong part: the stator's
/// outline against the planet blank's, which are not the same thing.
#[test]
fn compare_says_a_stale_dxf_is_not_this_part() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plate = repo().join("examples/parametric-plate.vcad");
    let out = dir.path().join("compare.json");
    let run = cam(&[
        "compare",
        "--dxf",
        fixture("stator-outline.dxf").to_str().expect("utf8"),
        "--outline",
        plate.to_str().expect("utf8"),
        "--tolerance",
        "0.02",
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(
        code(&run),
        2,
        "two different parts is the answer being no: {}",
        stderr(&run)
    );

    let a = answer(&out);
    assert_eq!(a["agrees"].as_bool(), Some(false));
    let apart = a["diff"]["max_boundary_distance"]
        .as_f64()
        .expect("a distance");
    assert!(
        apart > 1.0,
        "a stator DXF and an 80 x 50 plate are further apart than {apart} mm"
    );
    assert!(
        a["note"]
            .as_str()
            .expect("a sentence")
            .contains("wrong part"),
        "the note has to say what happens if you machine it anyway: {}",
        a["note"]
    );
}

/// …and the same outline against itself agrees, so the test above is measuring
/// a real disagreement rather than a comparison that always says no.
#[test]
fn compare_says_an_outline_is_itself() {
    let dir = tempfile::tempdir().expect("tempdir");
    let plate = repo().join("examples/parametric-plate.vcad");
    let sectioned = dir.path().join("outline.json");
    let run = cam(&[
        "outline",
        plate.to_str().expect("utf8"),
        "--out",
        sectioned.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));

    let a = answer(&sectioned);
    let request = serde_json::json!({
        "a": { "outline": a["outline"] },
        "b": { "outline": a["outline"] },
        "tolerance": 0.02,
    });
    let out = dir.path().join("compare.json");
    let run = cam(&[
        "compare",
        "--request",
        write(dir.path(), "same.json", &request)
            .to_str()
            .expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));
    let a = answer(&out);
    assert_eq!(a["agrees"].as_bool(), Some(true));
    assert!(
        a["diff"]["max_boundary_distance"]
            .as_f64()
            .expect("a distance")
            < 1e-9,
        "an outline is exactly itself"
    );
}

// ---------------------------------------------------------------------------
// fit, recommend
// ---------------------------------------------------------------------------

/// The Ø1 cutter fits the planet's tooth space, and the report says how much
/// bigger it could have been.
#[test]
fn fit_measures_the_planet_tooth_space() {
    let dir = tempfile::tempdir().expect("tempdir");
    // The space contour comes out of `gear`, so this also pins that the two
    // commands speak the same geometry.
    let gear_out = dir.path().join("gear.json");
    let run = cam(&[
        "gear",
        "--request",
        fixture("planet-gear.json").to_str().expect("utf8"),
        "--out",
        gear_out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));
    let gear = answer(&gear_out);
    let space = gear["contours"]["tooth_space"].clone();
    assert!(space.as_array().expect("points").len() > 20);

    let request = serde_json::json!({
        "contour": space, "tool_diameter": 1.0, "side": "inside",
    });
    let out = dir.path().join("fit.json");
    let run = cam(&[
        "fit",
        "--request",
        write(dir.path(), "fit-request.json", &request)
            .to_str()
            .expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));
    let a = answer(&out);
    assert_eq!(a["fits"].as_bool(), Some(true));
    let largest = a["largest_tool_diameter"].as_f64().expect("a diameter");
    assert!(
        (largest - 1.5575).abs() < 1e-3,
        "the largest cutter that fits this space is Ø{largest}, not Ø1.5575"
    );

    // `fits` is true, and the report still names two corners the cutter cannot
    // enter: the two ends of the space's MOUTH, 0.152 mm² each at 0.226 mm of
    // stand-off. That is not a contradiction — they are under the area the fit
    // report blocks on — but it is the same metal the job's `material_left`
    // check refuses, and it is why the rehearsal opens each space out past the
    // tip circle instead of cutting it as a closed loop.
    let unreachable = &a["report"]["unreachable"];
    assert_eq!(unreachable["count"].as_u64(), Some(2));
    let corners = unreachable["corners"].as_array().expect("two corners");
    for c in corners {
        let r =
            (c["centroid"][0].as_f64().expect("x")).hypot(c["centroid"][1].as_f64().expect("y"));
        assert!(
            r > 10.5,
            "an unreachable corner sits at r {r}, which is not the space's mouth (r ≈ 10.7)"
        );
    }
    let standoff = unreachable["max_standoff"].as_f64().expect("a stand-off");
    assert!(
        (standoff - 0.2258).abs() < 1e-3,
        "the worst stand-off at the mouth is {standoff} mm, not 0.2258"
    );

    // And a cutter that plainly does not fit is refused, so the pass above is
    // not a report that always says yes.
    let request = serde_json::json!({
        "contour": space, "tool_diameter": 3.0, "side": "inside",
    });
    let run = cam(&[
        "fit",
        "--request",
        write(dir.path(), "fit-big.json", &request)
            .to_str()
            .expect("utf8"),
        "--out",
        dir.path().join("fit-big-out.json").to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 2);
    assert!(stderr(&run).contains("does not fit"), "{}", stderr(&run));
}

/// The feeds the planet is cut at, including the one a dial router actually
/// needs: which number to turn the knob to.
#[test]
fn recommend_gives_brass_feeds_and_a_dial_position() {
    let dir = tempfile::tempdir().expect("tempdir");
    let request = serde_json::json!({
        "material": "brass-c360",
        "op": "slot",
        "tool": { "diameter": 1.0, "flutes": 2, "kind": "flat_end_mill", "flute_length": 6.0 },
        "machine": { "class": "hobby", "spindle": "dial", "max_feed": 4000 },
    });
    let out = dir.path().join("feeds.json");
    let run = cam(&[
        "recommend",
        "--request",
        write(dir.path(), "feeds-request.json", &request)
            .to_str()
            .expect("utf8"),
        "--out",
        out.to_str().expect("utf8"),
    ]);
    assert_eq!(code(&run), 0, "stderr: {}", stderr(&run));

    let r = &answer(&out)["recommendation"];
    assert_eq!(r["rpm"].as_f64(), Some(30000.0), "the spindle tops out");
    assert_eq!(
        r["dial"].as_str(),
        Some("6"),
        "on a dial router the S word does nothing, so the dial position is the recommendation"
    );
    let chipload = r["chipload_mm"].as_f64().expect("a chipload");
    assert!(
        (chipload - 0.0146 * 0.6).abs() < 1e-4,
        "hobby derates the table chipload by 60 %: {chipload}"
    );
    let feed = r["feed_mm_min"].as_f64().expect("a feed");
    assert!(
        (feed - chipload * 2.0 * 30000.0).abs() < 1e-6,
        "feed = chipload x flutes x rpm, and it came back {feed}"
    );
    assert!((r["stepdown_mm"].as_f64().expect("a stepdown") - 0.175).abs() < 1e-9);
}

// ---------------------------------------------------------------------------
// the request itself
// ---------------------------------------------------------------------------

/// A request that cannot be read at all is exit 1, not exit 2: the caller has
/// to be able to tell "fix your JSON" from "the part is not safe to cut".
#[test]
fn an_unreadable_request_is_not_a_refusal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = dir.path().join("bad.json");
    std::fs::write(&bad, "{ this is not json").expect("writable");
    let run = cam(&["job", "--request", bad.to_str().expect("utf8")]);
    assert_eq!(code(&run), 1, "stderr: {}", stderr(&run));

    let missing = dir.path().join("nowhere.json");
    let run = cam(&["job", "--request", missing.to_str().expect("utf8")]);
    assert_eq!(code(&run), 1);

    // …while a well-formed request the surface refuses by name is exit 2.
    let request = serde_json::json!({
        "stock": { "thickness": 0.0 },
        "tools": [{ "number": 1, "diameter": 1.0 }],
        "operations": [{ "tool": 1, "kind": "drill", "holes": [[0.0, 0.0]],
                         "depth": 1.0, "stepdown": 0.1, "feed": 100, "plunge": 50, "rpm": 10000 }],
    });
    let run = cam(&[
        "job",
        "--request",
        write(dir.path(), "zero-stock.json", &request)
            .to_str()
            .expect("utf8"),
    ]);
    assert_eq!(code(&run), 2);
    assert!(
        stderr(&run).contains("stock.thickness"),
        "a refusal names the field: {}",
        stderr(&run)
    );
}
