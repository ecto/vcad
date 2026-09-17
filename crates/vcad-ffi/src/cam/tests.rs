//! The CAM **C boundary**, exercised through the ABI it actually exports.
//!
//! The schema's own behaviour — what a job posts, which checks block, what the
//! oracle measures — is tested in `vcad-cam-api`, where the schema now lives.
//! What is left here is what only this crate can be wrong about: pointers,
//! ownership, the panic guard, the scene handle, and the legacy door the
//! shipped app still knocks on.
//!
//! Every test calls the `extern "C"` functions with a real `CString` and frees
//! the result with `vcad_cam_free`, so the ABI, the panic guard and the
//! ownership contract are all under test and not just the Rust behind them.
//! Nothing here wraps a call in `catch_unwind`: a panic that crosses the
//! boundary has to fail a test, not be absorbed by one.

use std::ffi::{c_char, CStr, CString};

use serde_json::{json, Value};
use vcad_kernel_cam::outline::{read_dxf, Loop};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Call a one-string-in, one-string-out entry point and parse the answer.
fn call(f: extern "C" fn(*const c_char) -> *mut c_char, request: &Value) -> Value {
    call_text(f, &request.to_string())
}

fn call_text(f: extern "C" fn(*const c_char) -> *mut c_char, request: &str) -> Value {
    let owned = CString::new(request).expect("request has no interior NUL");
    let raw = f(owned.as_ptr());
    assert!(
        !raw.is_null(),
        "a wave-2 entry point must never return null"
    );
    let text = unsafe { CStr::from_ptr(raw) }
        .to_str()
        .expect("the answer is UTF-8")
        .to_string();
    super::vcad_cam_free(raw);
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("the answer is not JSON: {e}\n{text}"))
}

fn ok(value: &Value) -> &Value {
    assert!(
        value.get("error").is_none(),
        "expected success, got: {}",
        value["error"]
    );
    value
}

fn f(value: &Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("not a number: {value}"))
}

fn fixture(name: &str) -> String {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/cam-fixtures")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The stator outline in the stock frame: the outer loop's lower-left corner
/// at the origin, exactly as the app's import places it.
struct Stator {
    outer: Vec<[f64; 2]>,
    /// Holes, largest first: the bore-and-slots loop, then the three pilots.
    holes: Vec<Vec<[f64; 2]>>,
}

fn stator() -> Stator {
    let outline = read_dxf(&fixture("stator-outline.dxf")).expect("the fixture DXF reads");
    let region = outline
        .regions
        .iter()
        .max_by(|a, b| a.outer.area().total_cmp(&b.outer.area()))
        .expect("the fixture has a region");
    let b = region.outer.bounds();
    let shift =
        |l: &Loop| -> Vec<[f64; 2]> { l.points.iter().map(|p| [p.x - b[0], p.y - b[1]]).collect() };
    let mut holes: Vec<Vec<[f64; 2]>> = region.holes.iter().map(shift).collect();
    holes.sort_by(|a, b| area(b).total_cmp(&area(a)));
    Stator {
        outer: shift(&region.outer),
        holes,
    }
}

fn area(points: &[[f64; 2]]) -> f64 {
    let mut a = 0.0;
    for i in 0..points.len() {
        let (p, q) = (points[i], points[(i + 1) % points.len()]);
        a += p[0] * q[1] - q[0] * p[1];
    }
    (a / 2.0).abs()
}

fn centroid(points: &[[f64; 2]]) -> [f64; 2] {
    let n = points.len() as f64;
    let (x, y) = points
        .iter()
        .fold((0.0, 0.0), |(x, y), p| (x + p[0], y + p[1]));
    [x / n, y / n]
}

/// Ø2 two-flute carbide — the cutter that survived the real job.
fn d2_tool(number: u32) -> Value {
    json!({ "number": number, "kind": "flat_end_mill", "diameter": 2.0,
            "flutes": 2, "flute_length": 6.0, "centre_cutting": true })
}

/// The job that was actually run, as the app posts it.
fn stator_job(allowance: f64, thickness: f64) -> Value {
    let s = stator();
    let mut operations = vec![json!({
        "name": "bore and slots", "tool": 1, "kind": "contour_inside",
        "contour": s.holes[0], "depth": 1.0, "stepdown": 0.17,
        "feed": 250, "plunge": 40, "rpm": 13500,
        "bottom_allowance": allowance,
    })];
    for (i, hole) in s.holes[1..].iter().enumerate() {
        let c = centroid(hole);
        operations.push(json!({
            "name": format!("pilot {}", i + 1), "tool": 1, "kind": "helical_bore",
            "x": c[0], "y": c[1], "diameter": 2.5, "pitch": 0.17,
            "depth": 1.0, "stepdown": 0.17,
            "feed": 250, "plunge": 40, "rpm": 13500,
            "bottom_allowance": allowance,
        }));
    }
    operations.push(json!({
        "name": "profile", "tool": 1, "kind": "contour_outside",
        "contour": s.outer, "depth": 1.0, "stepdown": 0.17,
        "feed": 250, "plunge": 40, "rpm": 13500,
        "tabs": 3, "tab_width": 4.0, "tab_height": 0.42,
        "bottom_allowance": allowance, "lead_in": false,
    }));
    json!({
        "name": "stator",
        "stock": { "thickness": thickness, "margin": 2.0 },
        "machine": { "name": "Anolex Ultra 2", "spindle": "dial" },
        "tools": [d2_tool(1)],
        "operations": operations,
        "options": { "part": { "outer": s.outer, "holes": s.holes } },
    })
}

// ---------------------------------------------------------------------------
// 1. The shared surface really is reachable through the ABI
// ---------------------------------------------------------------------------

#[test]
fn the_stator_job_crosses_the_abi_and_the_refusal_withholds_the_gcode() {
    // Not the whole verification — that is `vcad-cam-api`'s job — but the one
    // thing the C wrapper could silently break: a pass carries G-code across
    // the boundary, a refusal carries none.
    let pass = call(super::vcad_cam_job, &stator_job(0.15, 1.0));
    ok(&pass);
    assert_eq!(pass["blocked"], json!(false), "{}", pass["policy"]);
    assert!(
        pass["gcode"]
            .as_str()
            .is_some_and(|g| g.contains("M3") && g.contains("G1")),
        "a passing job has to hand real G-code across the ABI"
    );

    // The same job with no skin in stock thinner than the cut, over bare bed.
    let blocked = call(super::vcad_cam_job, &stator_job(0.0, 0.8));
    ok(&blocked);
    assert_eq!(blocked["blocked"], json!(true));
    assert!(
        blocked.get("gcode").is_none(),
        "a blocked job must not carry a gcode key at all"
    );
    // And the whole answer is free of anything an app could mistake for a
    // program: the point of the rule is that there is nothing to export.
    let text = blocked.to_string();
    assert!(
        !text.contains("G1 ") && !text.contains("M3"),
        "a blocked job's answer must not carry G-code anywhere in it"
    );
}

#[test]
fn every_entry_point_answers_and_the_string_it_returns_is_the_callers_to_free() {
    // A pass over the whole surface: each one answers JSON, and each answer
    // survives being freed — run twice so a double-free or a use-after-free
    // in the wrapper shows up as a crash rather than as a quiet pass.
    let s = stator();
    let requests: Vec<(&str, extern "C" fn(*const c_char) -> *mut c_char, Value)> = vec![
        ("vcad_cam_job", super::vcad_cam_job, stator_job(0.15, 1.0)),
        (
            "vcad_cam_fit",
            super::vcad_cam_fit,
            json!({ "contour": s.holes[0], "tool_diameter": 2.0, "side": "inside" }),
        ),
        (
            "vcad_cam_verify_gcode",
            super::vcad_cam_verify_gcode,
            json!({
                "gcode": fixture("stator-copper-d2.nc"),
                "part": { "outer": s.outer, "holes": s.holes },
                "stock": { "thickness": 1.0, "margin": 2.0 },
                "tool_diameter": 2.0, "bottom_allowance": 0.15,
            }),
        ),
        (
            "vcad_cam_compare_outline",
            super::vcad_cam_compare_outline,
            json!({ "a": { "loops": [s.outer] }, "b": { "loops": [s.outer] } }),
        ),
        (
            "vcad_cam_recommend",
            super::vcad_cam_recommend,
            json!({ "material": "copper-c110", "op": "slot",
                    "tool": { "diameter": 2.0, "flutes": 2 } }),
        ),
        (
            "vcad_cam_check_feeds",
            super::vcad_cam_check_feeds,
            json!({ "material": "copper-c110", "op": "slot",
                    "tool": { "diameter": 2.0, "flutes": 2 },
                    "settings": { "feed": 250, "plunge": 40, "rpm": 13500, "stepdown": 0.17 } }),
        ),
        (
            "vcad_cam_gear",
            super::vcad_cam_gear,
            json!({ "gear": { "module": 1.0, "teeth": 20 }, "cutter_diameter": 1.0,
                    "pin_diameter": 1.4, "contours": false }),
        ),
    ];
    for (name, entry, request) in &requests {
        for _ in 0..2 {
            let out = call(*entry, request);
            assert!(
                out.is_object(),
                "{name} answered something that is not a document: {out}"
            );
            ok(&out);
        }
    }

    // The no-argument one, twice, for the same reason.
    for _ in 0..2 {
        let raw = super::vcad_cam_materials();
        assert!(!raw.is_null());
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        super::vcad_cam_free(raw);
        let list: Value = serde_json::from_str(&text).unwrap();
        assert!(
            list["materials"].as_array().is_some_and(|m| m.len() >= 10),
            "the material table crossed the ABI short: {text}"
        );
    }
}

// ---------------------------------------------------------------------------
// 2. Nothing panics, nothing returns null
// ---------------------------------------------------------------------------

/// Every one-string entry point.
fn entry_points() -> Vec<(&'static str, extern "C" fn(*const c_char) -> *mut c_char)> {
    vec![
        ("vcad_cam_job", super::vcad_cam_job as _),
        ("vcad_cam_verify_gcode", super::vcad_cam_verify_gcode as _),
        ("vcad_cam_fit", super::vcad_cam_fit as _),
        (
            "vcad_cam_outline_from_mesh",
            super::vcad_cam_outline_from_mesh as _,
        ),
        (
            "vcad_cam_compare_outline",
            super::vcad_cam_compare_outline as _,
        ),
        ("vcad_cam_recommend", super::vcad_cam_recommend as _),
        ("vcad_cam_check_feeds", super::vcad_cam_check_feeds as _),
        ("vcad_cam_gear", super::vcad_cam_gear as _),
    ]
}

#[test]
fn rubbish_is_refused_in_words_and_a_null_request_is_not_a_crash() {
    let rubbish = ["", "{", "null", "[]", "\"a string\"", "{\"unknown\": 1}"];
    for (name, entry) in entry_points() {
        for text in rubbish {
            let out = call_text(entry, text);
            let message = out["error"]
                .as_str()
                .unwrap_or_else(|| panic!("{name} accepted {text:?}: {out}"));
            assert!(
                message.len() > 20 && message.ends_with('.'),
                "{name} on {text:?} answered {message:?}, which is not a sentence a machinist can act on"
            );
        }
        // A null pointer is not a crash either, and it names the call.
        let raw = entry(std::ptr::null());
        assert!(!raw.is_null(), "{name} returned null for a null request");
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        super::vcad_cam_free(raw);
        let out: Value = serde_json::from_str(&text).unwrap();
        assert!(out["error"].is_string(), "{name}: {text}");
    }
}

#[test]
fn a_null_scene_handle_is_an_error_not_a_crash() {
    let options = CString::new("{}").unwrap();
    let raw = super::vcad_cam_outline_from_scene(std::ptr::null(), 0, 0.0, 1, options.as_ptr());
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
    super::vcad_cam_free(raw);
    let out: Value = serde_json::from_str(&text).unwrap();
    assert!(out["error"].as_str().unwrap().contains("null"), "{text}");
}

// ---------------------------------------------------------------------------
// 3. The scene: the one entry point that cannot be shared
// ---------------------------------------------------------------------------

#[test]
fn a_scene_part_is_sectioned_from_its_raw_tessellation_not_its_export_mesh() {
    // The shipped example: an 80 x 50 x 6 plate with a Ø16 bore through it.
    // It is a boolean, so the export repair has something to do — and the
    // contour has to come from the tessellation that repair was *not* run on.
    let doc = include_str!("../../../../examples/parametric-plate.vcad");
    let scene = crate::vcad_scene_from_json(doc.as_ptr(), doc.len());
    assert!(!scene.is_null(), "the shipped example has to evaluate");

    let options = CString::new("{}").unwrap();
    let raw = super::vcad_cam_outline_from_scene(scene, 0, 0.0, 1, options.as_ptr());
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
    super::vcad_cam_free(raw);
    crate::vcad_scene_free(scene);
    let out: Value = serde_json::from_str(&text).unwrap();
    ok(&out);

    assert_eq!(
        out["mesh_source"],
        json!("raw_tessellation"),
        "a part with a B-rep behind it must not be sectioned from the export mesh"
    );
    // Auto z is the mid-height of the part's own bounds, and the bounds are
    // the stock thickness to suggest.
    assert!((f(&out["z"]) - 3.0).abs() < 1e-6, "{}", out["z"]);
    assert!(
        (f(&out["suggested_stock_thickness"]) - 6.0).abs() < 1e-6,
        "a 6 mm plate suggests 6 mm of stock, not {}",
        out["suggested_stock_thickness"]
    );

    // The geometry, not merely "a contour came back": 80 x 50 outside, one
    // Ø16 hole.
    let bounds: Vec<f64> = serde_json::from_value(out["bounds"].clone()).unwrap();
    assert!(
        (bounds[2] - bounds[0] - 80.0).abs() < 0.01 && (bounds[3] - bounds[1] - 50.0).abs() < 0.01,
        "the section is {:.3} x {:.3}, not 80 x 50",
        bounds[2] - bounds[0],
        bounds[3] - bounds[1]
    );
    let circles = out["circles"].as_array().unwrap();
    assert_eq!(circles.len(), 1, "one bore, got {circles:?}");
    let d = f(&circles[0]["diameter"]);
    assert!(
        (d - 16.0).abs() < 0.05,
        "the bore measures Ø{d:.4}, not Ø16"
    );
}

#[test]
fn a_part_index_past_the_end_of_the_scene_is_named_not_guessed() {
    let doc = include_str!("../../../../examples/parametric-plate.vcad");
    let scene = crate::vcad_scene_from_json(doc.as_ptr(), doc.len());
    assert!(!scene.is_null());
    let options = CString::new("{}").unwrap();
    let raw = super::vcad_cam_outline_from_scene(scene, 99, 0.0, 1, options.as_ptr());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
    super::vcad_cam_free(raw);
    crate::vcad_scene_free(scene);
    let out: Value = serde_json::from_str(&text).unwrap();
    let message = out["error"].as_str().expect("there is no part 99");
    assert!(
        message.contains("part 99"),
        "the refusal has to name the index asked for: {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 4. The old door still opens
// ---------------------------------------------------------------------------

#[test]
fn the_legacy_entry_point_is_unchanged() {
    let request = json!({ "operation": "pocket", "width": 40, "height": 30, "depth": 1,
                          "diameter": 3.175, "stepdown": 0.5, "stepover": 1.5, "feed": 400,
                          "plunge": 100, "rpm": 10000, "clearance": 5 });
    let owned = CString::new(request.to_string()).unwrap();
    let raw = super::vcad_cam_generate(owned.as_ptr());
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
    super::vcad_cam_free(raw);
    let out: Value = serde_json::from_str(&text).unwrap();
    let gcode = out["gcode"].as_str().unwrap();
    assert!(
        gcode.ends_with("M5\nM2\n"),
        "the legacy footer is unchanged"
    );
    assert!(!gcode.contains("M6"));
    assert!(out["moves"].as_array().unwrap().len() > 20);
    // Its contract is still null-on-failure, not an error document.
    let bad = CString::new("{}").unwrap();
    assert!(super::vcad_cam_generate(bad.as_ptr()).is_null());
}

/// The part this roadmap started with, solved by the kernel and sectioned
/// through the C ABI. `VCAD_STATOR_VCAD` points at the evaluated document
/// (`loon2vcad stator.loon`); the solve takes ~20 s, so it is opt-in.
#[test]
#[ignore = "needs VCAD_STATOR_VCAD and ~20 s"]
fn the_real_stator_gives_its_own_contour() {
    let Ok(path) = std::env::var("VCAD_STATOR_VCAD") else {
        panic!("set VCAD_STATOR_VCAD");
    };
    let doc = std::fs::read_to_string(path).unwrap();
    let scene = crate::vcad_scene_from_json(doc.as_ptr(), doc.len());
    assert!(!scene.is_null(), "the stator has to evaluate");
    let mut report = Vec::new();
    for z in [11.4, 13.0, 14.1, 16.8] {
        // `VCAD_STATOR_HEAL` (mm) loosens the default 1e-3 heal: until the
        // tangent tab-corner seam is closed in the kernel the raw solid
        // carries two 0.015 mm cracks, and the default rightly refuses them.
        let heal = std::env::var("VCAD_STATOR_HEAL").ok();
        let options = CString::new(match &heal {
            Some(h) => format!("{{\"heal_tolerance\": {h}}}"),
            None => "{}".to_string(),
        })
        .unwrap();
        let raw = super::vcad_cam_outline_from_scene(scene, 0, z, 0, options.as_ptr());
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        super::vcad_cam_free(raw);
        let out: Value = serde_json::from_str(&text).unwrap();
        if out.get("error").is_some() {
            report.push(format!("z {z}: REFUSED {}", out["error"]));
            continue;
        }
        let compare = call(
            super::vcad_cam_compare_outline,
            &json!({ "dxf": fixture("stator-outline.dxf"), "outline": out["outline"], "tolerance": 0.02 }),
        );
        report.push(format!(
            "z {z}: {} | holes {} | circles {} | max boundary {:.5} mm | agrees {}",
            out["mesh_source"],
            out["outline"]["regions"][0]["holes"]
                .as_array()
                .map_or(0, Vec::len),
            out["circles"].as_array().map_or(0, Vec::len),
            f(&compare["diff"]["max_boundary_distance"]),
            compare["agrees"]
        ));
    }
    crate::vcad_scene_free(scene);
    for line in &report {
        println!("{line}");
    }
    assert!(
        report.iter().all(|l| l.contains("agrees true")),
        "the contour taken from the solid is not the part:\n{}",
        report.join("\n")
    );
}
