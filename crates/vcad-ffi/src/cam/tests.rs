//! The CAM FFI surface, exercised through the C ABI it actually exports.
//!
//! House rule 1 applies: these assert *geometry* — sides, diameters, depths,
//! the envelope the cutter sweeps, which checks blocked — never that a call
//! came back non-empty. The fixture is the part that was really cut on
//! 2026-09-17: the rana-60 stator, 1 mm copper, Ø2 two-flute.
//!
//! Every test calls the `extern "C"` functions with a real `CString` and frees
//! the result with `vcad_cam_free`, so the ABI, the panic guard and the
//! ownership contract are all under test and not just the Rust behind them.
//! Nothing here wraps a call in `catch_unwind`: a panic that crosses the
//! boundary has to fail a test, not be absorbed by one.

use std::ffi::{c_char, CStr, CString};

use serde_json::{json, Value};
use vcad_kernel_cam::outline::{read_dxf, Loop, Outline};

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

// ---------------------------------------------------------------------------
// The stator fixture
// ---------------------------------------------------------------------------

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
    bounds: [f64; 4],
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
        bounds: [0.0, 0.0, b[2] - b[0], b[3] - b[1]],
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

/// The job that was actually run: bore and slots, three pilots, outside
/// profile with three tabs, 1 mm stock, a 0.15 mm onion skin.
fn stator_job(allowance: f64, thickness: f64, spoilboard: Option<f64>) -> Value {
    let s = stator();
    let mut stock = json!({ "thickness": thickness, "margin": 2.0 });
    if let Some(t) = spoilboard {
        stock["spoilboard"] = json!(t);
    }
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
        "stock": stock,
        "machine": { "name": "Anolex Ultra 2", "spindle": "dial" },
        "tools": [d2_tool(1)],
        "operations": operations,
        "options": { "part": { "outer": s.outer, "holes": s.holes } },
    })
}

// ---------------------------------------------------------------------------
// 1. The stator, end to end
// ---------------------------------------------------------------------------

#[test]
fn the_stator_job_posts_one_spindle_start_and_verifies_clean() {
    let out = call(super::vcad_cam_job, &stator_job(0.15, 1.0, None));
    ok(&out);
    assert_eq!(
        out["blocked"],
        json!(false),
        "the job that was really cut must not be refused: {}",
        out["policy"]
    );
    assert_eq!(
        out["verification"]["pass"],
        json!(true),
        "verification: {}",
        out["policy"]
    );

    let gcode = out["gcode"].as_str().expect("a passing job carries gcode");
    // One tool means one spindle start, one spin-up dwell, and no operator
    // stop at all — friction-log item 42, which used to restart the spindle
    // between every operation.
    assert_eq!(count_word(gcode, "M3"), 1, "exactly one M3:\n{gcode}");
    assert_eq!(count_word(gcode, "G4"), 1, "exactly one spin-up dwell");
    assert_eq!(count_word(gcode, "M0"), 0, "a single-tool job never stops");

    // The three pilots are bored, not drilled: the wall each one leaves has
    // to measure Ø2.5, cut with a Ø2 cutter.
    let s = stator();
    for (i, hole) in s.holes[1..].iter().enumerate() {
        let c = centroid(hole);
        let range = out["op_ranges"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == json!(format!("pilot {}", i + 1)))
            .expect("every operation has a range");
        let moves = out["moves"].as_array().unwrap();
        let (start, end) = (f(&range["start"]) as usize, f(&range["end"]) as usize);
        // The widest the tool centre gets from the bore centre, plus the
        // radius, is the wall the cutter leaves.
        let widest = moves[start..end]
            .iter()
            .filter(|m| m["rapid"] == json!(false))
            .map(|m| {
                let to = m["to"].as_array().unwrap();
                (f(&to[0]) - c[0]).hypot(f(&to[1]) - c[1])
            })
            .fold(0.0f64, f64::max);
        let diameter = 2.0 * (widest + 1.0);
        assert!(
            (diameter - 2.5).abs() < 0.01,
            "pilot {}: the bore measures Ø{diameter:.4}, not Ø2.5",
            i + 1
        );
    }

    // The sweep, cutter included: the part is 63.15 mm and a Ø2 cutter runs
    // one radius outside it, so the metal it passes over runs -2.00 .. 65.15.
    let env = &out["verification"]["envelope"];
    for axis in 0..2 {
        assert!(
            (f(&env["work_min"][axis]) + 2.00).abs() < 0.02,
            "envelope min on axis {axis} is {}, expected -2.00",
            env["work_min"][axis]
        );
        assert!(
            (f(&env["work_max"][axis]) - 65.15).abs() < 0.02,
            "envelope max on axis {axis} is {}, expected 65.15",
            env["work_max"][axis]
        );
    }

    // The onion skin is really left: nothing goes below -0.85.
    let deepest = f(&out["verification"]["depth"]["deepest_z"]);
    assert!(
        (-0.851..=-0.849).contains(&deepest),
        "the deepest cut is {deepest:.4}, not the -0.85 a 0.15 mm skin in 1 mm stock leaves"
    );

    // Three tabs, audited where they are, not where they were asked for.
    assert_eq!(out["verification"]["tabs"]["tab_count"], json!(3));

    // The envelope above is only the right number because the part is the
    // size the friction log recorded; pin that too.
    assert!(
        (s.bounds[2] - 63.15).abs() < 0.01 && (s.bounds[3] - 63.15).abs() < 0.01,
        "the stator fixture is {:.3} x {:.3} mm, not 63.15 square",
        s.bounds[2],
        s.bounds[3]
    );
}

fn count_word(gcode: &str, word: &str) -> usize {
    gcode
        .lines()
        .map(|l| l.split('(').next().unwrap_or(""))
        .filter(|l| {
            l.split_whitespace()
                .any(|w| w.eq_ignore_ascii_case(word) || strip_number(w) == word)
        })
        .count()
}

/// `G4P3.000` and `G4 P3` are the same word.
fn strip_number(word: &str) -> String {
    let mut out = String::new();
    for c in word.chars() {
        if c.is_ascii_alphabetic() {
            if !out.is_empty() {
                break;
            }
            out.push(c.to_ascii_uppercase());
        } else if out.is_empty() {
            return String::new();
        } else if c.is_ascii_digit() {
            out.push(c);
        } else {
            break;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// 2. Fail closed
// ---------------------------------------------------------------------------

#[test]
fn cutting_past_the_underside_with_nothing_beneath_is_refused_and_no_gcode_comes_back() {
    // The same job, no skin, in stock thinner than the cut, over bare bed.
    let out = call(super::vcad_cam_job, &stator_job(0.0, 0.8, None));
    ok(&out);
    assert_eq!(
        out["blocked"],
        json!(true),
        "a 1.0 mm cut in 0.8 mm stock over nothing has to be refused"
    );
    let blocked_by: Vec<String> = serde_json::from_value(out["policy"]["blocked_by"].clone())
        .expect("blocked_by is a list of check names");
    assert!(
        blocked_by.iter().any(|c| c == "depth"),
        "the refusal has to name the depth check, got {blocked_by:?}"
    );
    // The point of the rule: there is nothing to export.
    assert!(
        out.get("gcode").is_none(),
        "a blocked job must not carry a gcode key at all"
    );
    assert!(
        out["verification"].is_object(),
        "a blocked job still has to say what it found"
    );
}

#[test]
fn a_break_through_needs_a_board_under_it_and_pecks_when_it_is_told_to() {
    // Item 40 and item 50 in one job: a cut that goes past the underside is
    // legal only into a declared spoilboard, and there was no way to say what
    // was under the stock at all.
    let s = stator();
    let holes: Vec<Value> = s.holes[1..]
        .iter()
        .map(|h| {
            let c = centroid(h);
            json!({ "x": c[0], "y": c[1] })
        })
        .collect();
    let drill = |spoilboard: Option<f64>| {
        let mut stock = json!({ "thickness": 1.0, "margin": 2.0 });
        if let Some(t) = spoilboard {
            stock["spoilboard"] = json!(t);
        }
        json!({
            "name": "pilots",
            "stock": stock,
            "tools": [{ "number": 1, "kind": "drill", "diameter": 2.5, "angle": 118.0 }],
            "operations": [{
                "name": "pilots", "tool": 1, "kind": "drill", "holes": holes,
                "depth": 1.0, "stepdown": 0.5, "feed": 120, "plunge": 40, "rpm": 6000,
                "cycle": "peck", "peck_depth": 0.3,
                // Negative is a deliberate break-through, 0.3 mm past the back.
                "bottom_allowance": -0.3, "through": true,
            }],
            "options": { "verify": false, "part": { "outer": s.outer, "holes": s.holes } },
        })
    };

    // Over a bare bed: refused, by name, before any geometry is built.
    let bare = call(super::vcad_cam_job, &drill(None));
    let message = bare["error"]
        .as_str()
        .expect("a break-through over nothing has to be refused");
    assert!(
        message.contains("0.300 mm past the underside") && message.contains("spoilboard"),
        "the refusal has to say how far past and what is missing: {message:?}"
    );

    // Over 3 mm of MDF: allowed, and the drill really pecks.
    let over_board = call(super::vcad_cam_job, &drill(Some(3.0)));
    ok(&over_board);
    assert_eq!(over_board["blocked"], json!(false));
    let moves = over_board["moves"].as_array().unwrap();
    let deepest = moves
        .iter()
        .map(|m| f(&m["to"][2]))
        .fold(f64::MAX, f64::min);
    // 1 mm of stock, a 118° point on Ø2.5 (0.751 mm), and 0.3 mm beyond.
    assert!(
        (deepest + 2.051).abs() < 0.01,
        "the drill reaches {deepest:.4}, not the -2.051 that 1 mm + point + 0.3 mm asks for"
    );
    // A peck retracts to clearance between bites, so Z goes up and down many
    // times inside one hole rather than straight down.
    let reversals = moves
        .windows(3)
        .filter(|w| {
            let (a, b, c) = (f(&w[0]["to"][2]), f(&w[1]["to"][2]), f(&w[2]["to"][2]));
            (b - a).signum() != (c - b).signum() && (b - a).abs() > 1e-9 && (c - b).abs() > 1e-9
        })
        .count();
    assert!(
        reversals >= 3 * 7,
        "three holes pecked 0.3 mm at a time should reverse in Z far more than {reversals} times"
    );
}

#[test]
fn an_inside_contour_offset_the_wrong_way_is_refused_for_gouging() {
    // Friction-log item 32: `inside` used to be byte-identical to `outside`,
    // and the bore pass ran 3.2 mm into every post. Build that mistake on
    // purpose — the loop pushed outward by hand — and the oracle must catch
    // it, with a worst case of about one tool diameter.
    let s = stator();
    let c = centroid(&s.holes[0]);
    let wrong: Vec<[f64; 2]> = s.holes[0]
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            let r = dx.hypot(dy).max(1e-9);
            [p[0] + 2.0 * dx / r, p[1] + 2.0 * dy / r]
        })
        .collect();
    let request = json!({
        "name": "wrong side",
        "stock": { "thickness": 1.0, "margin": 2.0 },
        "tools": [d2_tool(1)],
        "operations": [{
            "name": "bore and slots", "tool": 1, "kind": "contour_inside",
            "contour": wrong, "depth": 1.0, "stepdown": 0.17,
            "feed": 250, "plunge": 40, "rpm": 13500, "bottom_allowance": 0.15,
        }],
        "options": { "part": { "outer": s.outer, "holes": s.holes } },
    });
    let out = call(super::vcad_cam_job, &request);
    ok(&out);
    assert_eq!(out["blocked"], json!(true), "{}", out["policy"]);
    let blocked_by: Vec<String> =
        serde_json::from_value(out["policy"]["blocked_by"].clone()).unwrap();
    assert!(
        blocked_by.iter().any(|c| c == "gouge"),
        "the refusal has to name the gouge check, got {blocked_by:?}"
    );
    assert!(out.get("gcode").is_none());
    let worst = f(&out["verification"]["gouge"]["worst"]);
    assert!(
        (1.0..=3.5).contains(&worst),
        "a loop pushed 2 mm into the wall with a Ø2 cutter should gouge by about a tool diameter, not {worst:.3} mm"
    );
}

// ---------------------------------------------------------------------------
// 3. Two tools
// ---------------------------------------------------------------------------

/// T1 Ø3.175 cuts the outside profile; T2 Ø2 does everything inside, which is
/// the split the real job wanted and could not have.
fn two_tool_job() -> Value {
    let s = stator();
    let mut operations = vec![
        json!({
            "name": "profile", "tool": 1, "kind": "contour_outside",
            "contour": s.outer, "depth": 1.0, "stepdown": 0.3,
            "feed": 400, "plunge": 60, "rpm": 13500,
            "tabs": 3, "tab_width": 4.0, "tab_height": 0.42,
            "bottom_allowance": 0.15, "lead_in": false,
        }),
        json!({
            "name": "bore and slots", "tool": 2, "kind": "contour_inside",
            "contour": s.holes[0], "depth": 1.0, "stepdown": 0.17,
            "feed": 250, "plunge": 40, "rpm": 13500, "bottom_allowance": 0.15,
        }),
    ];
    for (i, hole) in s.holes[1..].iter().enumerate() {
        let c = centroid(hole);
        operations.push(json!({
            "name": format!("pilot {}", i + 1), "tool": 2, "kind": "helical_bore",
            "x": c[0], "y": c[1], "diameter": 2.5, "pitch": 0.17,
            "depth": 1.0, "stepdown": 0.17,
            "feed": 250, "plunge": 40, "rpm": 13500, "bottom_allowance": 0.15,
        }));
    }
    json!({
        "name": "stator two tools",
        "stock": { "thickness": 1.0, "margin": 2.0 },
        "tools": [
            { "number": 1, "kind": "flat_end_mill", "diameter": 3.175, "flutes": 2,
              "flute_length": 12.0, "centre_cutting": true },
            d2_tool(2),
        ],
        "operations": operations,
        "options": {
            "tool_change": { "type": "manual_pause_reprobe" },
            "part": { "outer": s.outer, "holes": s.holes },
        },
    })
}

#[test]
fn a_two_tool_job_stops_once_and_its_ranges_partition_the_preview() {
    let out = call(super::vcad_cam_job, &two_tool_job());
    ok(&out);
    assert_eq!(out["blocked"], json!(false), "{}", out["policy"]);
    let gcode = out["gcode"].as_str().expect("gcode");
    assert_eq!(
        count_word(gcode, "M0"),
        1,
        "one manual tool change is one stop, no more and no less:\n{gcode}"
    );
    assert_eq!(count_word(gcode, "M3"), 2, "one spindle start per tool");

    // Inside features before the profile that frees the part, whatever order
    // they were typed in.
    let ranges = out["op_ranges"].as_array().unwrap();
    let ops: Vec<&str> = ranges
        .iter()
        .filter(|r| r["block"] == json!("operation"))
        .map(|r| r["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        ops.last().copied(),
        Some("profile"),
        "the outside profile has to run last, got {ops:?}"
    );

    // The ranges partition the preview: contiguous, from 0, covering it all.
    let moves = out["moves"].as_array().unwrap().len();
    let mut cursor = 0usize;
    for r in ranges {
        assert_eq!(
            f(&r["start"]) as usize,
            cursor,
            "block {} starts at {} but the previous one ended at {cursor}",
            r["block"],
            r["start"]
        );
        cursor = f(&r["end"]) as usize;
    }
    assert_eq!(cursor, moves, "the ranges have to cover every move");
}

// ---------------------------------------------------------------------------
// 4. Arc fitting
// ---------------------------------------------------------------------------

#[test]
fn arc_fitting_keeps_the_verdict_and_costs_fewer_moves() {
    let plain = call(super::vcad_cam_job, &stator_job(0.15, 1.0, None));
    let mut with_arcs = stator_job(0.15, 1.0, None);
    with_arcs["options"]["arc_fit"] = json!({ "tolerance": 0.01 });
    let fitted = call(super::vcad_cam_job, &with_arcs);
    ok(&plain);
    ok(&fitted);

    assert_eq!(
        plain["verification"]["pass"], fitted["verification"]["pass"],
        "fitting arcs must not change the verdict"
    );
    assert_eq!(plain["blocked"], fitted["blocked"]);

    let before = plain["moves"].as_array().unwrap().len();
    let after = fitted["moves"].as_array().unwrap().len();
    assert!(
        after < before,
        "fitting arcs has to shorten the program: {before} moves became {after}"
    );
    let deviation = f(&fitted["arc_fit"]["max_deviation"]);
    assert!(
        deviation <= 0.01 + 1e-9,
        "an arc fit at 0.01 mm may not deviate by {deviation:.6} mm"
    );
    assert!(
        f(&fitted["arc_fit"]["arcs_emitted"]) > 0.0,
        "the stator is full of arcs; none were fitted"
    );
}

// ---------------------------------------------------------------------------
// 5. Verifying a file off disk
// ---------------------------------------------------------------------------

#[test]
fn the_real_copper_program_verifies_against_the_outline_it_was_cut_from() {
    let s = stator();
    let request = json!({
        "gcode": fixture("stator-copper-d2.nc"),
        "part": { "outer": s.outer, "holes": s.holes },
        "stock": { "thickness": 1.0, "margin": 2.0 },
        "tool_diameter": 2.0,
        "bottom_allowance": 0.15,
        // Three tabs were cut, so three are declared: the audit compares what
        // was asked for against what the program actually leaves standing.
        "tabs": [{ "width": 4.0, "height": 0.42 }, { "width": 4.0, "height": 0.42 },
                 { "width": 4.0, "height": 0.42 }],
    });
    let out = call(super::vcad_cam_verify_gcode, &request);
    ok(&out);
    assert_eq!(
        out["pass"],
        json!(true),
        "the program that cut a clean part has to verify: {}",
        out["policy"]
    );
    assert_eq!(out["blocked"], json!(false));
    assert!(
        f(&out["verification"]["moves"]) > 1000.0,
        "the real file is thousands of moves; only {} were replayed",
        out["verification"]["moves"]
    );
}

// ---------------------------------------------------------------------------
// 6. Outline out of a solid
// ---------------------------------------------------------------------------

/// Extrude an outline's loops into the side walls of a prism. Walls alone are
/// all a Z section ever touches, and they carry the tear a bad mesh would
/// have.
fn extrude(outline: &Outline, z0: f64, z1: f64) -> (Vec<[f64; 3]>, Vec<u32>) {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for l in outline.loops() {
        let base = positions.len() as u32;
        for p in &l.points {
            positions.push([p.x, p.y, z0]);
            positions.push([p.x, p.y, z1]);
        }
        let n = l.points.len() as u32;
        for i in 0..n {
            let (a, b) = (base + 2 * i, base + 2 * ((i + 1) % n));
            indices.extend_from_slice(&[a, b, a + 1, b, b + 1, a + 1]);
        }
    }
    (positions, indices)
}

#[test]
fn sectioning_an_extruded_stator_gives_back_the_outline_it_was_extruded_from() {
    let dxf = read_dxf(&fixture("stator-outline.dxf")).unwrap();
    let (positions, indices) = extrude(&dxf, 0.0, 6.0);
    let out = call(
        super::vcad_cam_outline_from_mesh,
        &json!({ "positions": positions, "indices": indices, "auto_z": true }),
    );
    ok(&out);
    assert_eq!(out["mesh_source"], json!("inline"));
    assert!(
        (f(&out["z"]) - 3.0).abs() < 1e-6,
        "auto z has to land mid-height, got {}",
        out["z"]
    );
    assert!(
        (f(&out["suggested_stock_thickness"]) - 6.0).abs() < 1e-9,
        "the stock thickness suggestion is the part's own Z range"
    );

    // One region with four holes: the bore-and-slots loop and three pilots.
    let regions = out["regions"].as_array().unwrap();
    assert_eq!(regions.len(), 1, "the stator is one region");
    assert_eq!(
        regions[0]["holes"].as_array().unwrap().len(),
        4,
        "four holes: bore-and-slots and three pilots"
    );

    // Three of them are Ø2.5 circles.
    let circles: Vec<f64> = out["circles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| f(&c["diameter"]))
        .collect();
    let pilots: Vec<f64> = circles
        .iter()
        .copied()
        .filter(|d| (d - 2.5).abs() < 0.01)
        .collect();
    assert_eq!(
        pilots.len(),
        3,
        "expected three Ø2.5 pilots among the circles {circles:?}"
    );

    // And it is the same outline the DXF describes.
    let compare = call(
        super::vcad_cam_compare_outline,
        &json!({ "dxf": fixture("stator-outline.dxf"), "outline": out["outline"], "tolerance": 0.02 }),
    );
    ok(&compare);
    assert_eq!(compare["agrees"], json!(true), "{}", compare["note"]);
    let distance = f(&compare["diff"]["max_boundary_distance"]);
    assert!(
        distance < 0.02,
        "the section is {distance:.5} mm off the DXF it came from"
    );
    assert_eq!(
        compare["diff"]["hole_count_a"],
        compare["diff"]["hole_count_b"]
    );

    // A constant-section part is what a contour job assumes; say so.
    assert_eq!(
        out["prismatic"]["prismatic"],
        json!(true),
        "an extruded prism is prismatic: {}",
        out["prismatic"]
    );
}

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
fn a_stale_outline_is_caught_before_it_machines_the_wrong_part() {
    // Item 16, in one call: the same part with one pilot missing is not the
    // same part, and nothing about its bounds says so.
    let dxf = read_dxf(&fixture("stator-outline.dxf")).unwrap();
    let mut stale = dxf.clone();
    stale.regions[0].holes.pop();
    let compare = call(
        super::vcad_cam_compare_outline,
        &json!({ "a": { "outline": dxf }, "b": { "outline": stale }, "tolerance": 0.02 }),
    );
    ok(&compare);
    assert_eq!(compare["agrees"], json!(false), "{}", compare["note"]);
    assert_eq!(
        compare["diff"]["unmatched_a"].as_array().unwrap().len(),
        1,
        "the missing hole has to be named"
    );
}

#[test]
#[ignore = "prints the measured numbers; not an assertion"]
fn zz_numbers() {
    let plain = call(super::vcad_cam_job, &stator_job(0.15, 1.0, None));
    let mut a = stator_job(0.15, 1.0, None);
    a["options"]["arc_fit"] = json!({ "tolerance": 0.01 });
    let fitted = call(super::vcad_cam_job, &a);
    println!(
        "ARCFIT moves {} -> {} ; arcs {} ; dev {} ; segs {} -> {}",
        plain["moves"].as_array().unwrap().len(),
        fitted["moves"].as_array().unwrap().len(),
        fitted["arc_fit"]["arcs_emitted"],
        fitted["arc_fit"]["max_deviation"],
        fitted["arc_fit"]["segments_in"],
        fitted["arc_fit"]["segments_out"],
    );
    println!("DURATION {}", plain["duration"]);
    println!(
        "ENVELOPE min {} max {}",
        plain["verification"]["envelope"]["work_min"],
        plain["verification"]["envelope"]["work_max"]
    );
    println!("DEEPEST {}", plain["verification"]["depth"]["deepest_z"]);
    let s = stator();
    let c = centroid(&s.holes[0]);
    let w: Vec<[f64; 2]> = s.holes[0]
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            let r = dx.hypot(dy).max(1e-9);
            [p[0] + 2.0 * dx / r, p[1] + 2.0 * dy / r]
        })
        .collect();
    let wrong = call(
        super::vcad_cam_job,
        &json!({
            "stock": { "thickness": 1.0, "margin": 2.0 },
            "tools": [d2_tool(1)],
            "operations": [{ "name": "bore", "tool": 1, "kind": "contour_inside",
                "contour": w, "depth": 1.0, "stepdown": 0.17,
                "feed": 250, "plunge": 40, "rpm": 13500, "bottom_allowance": 0.15 }],
            "options": { "part": { "outer": s.outer, "holes": s.holes } },
        }),
    );
    println!("GOUGE worst {}", wrong["verification"]["gouge"]["worst"]);
    for d in [2.0, 3.175] {
        let r = call(
            super::vcad_cam_fit,
            &json!({ "contour": s.holes[0], "tool_diameter": d, "side": "inside" }),
        );
        println!(
            "FIT d{d} area {} standoff {} count {} largest {}",
            r["report"]["unreachable"]["total_area"],
            r["report"]["unreachable"]["max_standoff"],
            r["report"]["unreachable"]["count"],
            r["largest_tool_diameter"]
        );
    }
    let two = call(super::vcad_cam_job, &two_tool_job());
    println!(
        "TWOTOOL moves {} ranges {} policy {}",
        two["moves"].as_array().unwrap().len(),
        two["op_ranges"].as_array().unwrap().len(),
        two["policy"]
    );
    let vg = call(
        super::vcad_cam_verify_gcode,
        &json!({
            "gcode": fixture("stator-copper-d2.nc"),
            "part": { "outer": s.outer, "holes": s.holes },
            "stock": { "thickness": 1.0, "margin": 2.0 },
            "tool_diameter": 2.0, "bottom_allowance": 0.15,
            "tabs": [{ "width": 4.0, "height": 0.42 }, { "width": 4.0, "height": 0.42 },
                     { "width": 4.0, "height": 0.42 }],
        }),
    );
    println!(
        "NCFILE pass {} moves {}",
        vg["pass"], vg["verification"]["moves"]
    );
    let dxf = read_dxf(&fixture("stator-outline.dxf")).unwrap();
    let (positions, indices) = extrude(&dxf, 0.0, 6.0);
    let sec = call(
        super::vcad_cam_outline_from_mesh,
        &json!({ "positions": positions, "indices": indices, "auto_z": true }),
    );
    let cmp = call(
        super::vcad_cam_compare_outline,
        &json!({ "dxf": fixture("stator-outline.dxf"), "outline": sec["outline"], "tolerance": 0.02 }),
    );
    println!(
        "SECTION holes {} circles {} maxdist {} symdiff {}",
        sec["regions"][0]["holes"].as_array().unwrap().len(),
        sec["circles"],
        cmp["diff"]["max_boundary_distance"],
        cmp["diff"]["symmetric_difference_area"]
    );
}

// ---------------------------------------------------------------------------
// 7. Cutter fit
// ---------------------------------------------------------------------------

#[test]
fn the_cutter_fit_report_names_the_corners_a_bigger_tool_cannot_reach() {
    // Item 38: the stator was drawn for a Ø2 cutter (R1.05 inside fillets).
    // A Ø2 cutter reaches those corners; a Ø3.175 one leaves metal in them.
    let s = stator();
    let small = call(
        super::vcad_cam_fit,
        &json!({ "contour": s.holes[0], "tool_diameter": 2.0, "side": "inside" }),
    );
    let large = call(
        super::vcad_cam_fit,
        &json!({ "contour": s.holes[0], "tool_diameter": 3.175, "side": "inside" }),
    );
    ok(&small);
    ok(&large);
    let small_area = f(&small["report"]["unreachable"]["total_area"]);
    let large_area = f(&large["report"]["unreachable"]["total_area"]);
    assert!(
        large_area > small_area,
        "a Ø3.175 cutter cannot leave less metal in the corners ({large_area:.4} mm²) than a Ø2 one ({small_area:.4} mm²)"
    );
    assert!(
        small_area < 0.05,
        "the part was drawn for Ø2: it should leave essentially nothing, not {small_area:.4} mm²"
    );
    assert!(
        f(&small["report"]["unreachable"]["max_standoff"]) < 0.02,
        "a Ø2 cutter reaches the R1.05 fillets"
    );

    // And it reproduces what was counted by hand on the day: 24 inside
    // corners keeping up to 0.29 mm, about 10.8 mm² in all.
    assert_eq!(
        large["report"]["unreachable"]["count"],
        json!(24),
        "the friction log counted 24 inside corners a Ø3.175 cutter misses"
    );
    assert!(
        (large_area - 10.6).abs() < 0.5,
        "those corners hold about 10.8 mm² of metal, not {large_area:.4}"
    );
    let standoff = f(&large["report"]["unreachable"]["max_standoff"]);
    assert!(
        (standoff - 0.29).abs() < 0.01,
        "the worst of them stands off 0.29 mm, not {standoff:.4}"
    );

    // Item 36's other number: Ø3.175 passes the 3.87 mm slot mouths and
    // anything from about Ø3.9 up does not.
    let largest = f(&small["largest_tool_diameter"]);
    assert!(
        (3.8..3.9).contains(&largest),
        "the largest cutter that still fits is Ø{largest:.4}, not the Ø3.87 the slot mouths allow"
    );
    assert_eq!(
        small["largest_tool_diameter"], large["largest_tool_diameter"],
        "the largest cutter that fits is a property of the contour, not of the tool asked about"
    );
}

// ---------------------------------------------------------------------------
// 8. Materials
// ---------------------------------------------------------------------------

#[test]
fn the_material_table_round_trips_and_reproduces_the_copper_anchor() {
    let raw = super::vcad_cam_materials();
    assert!(!raw.is_null());
    let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
    super::vcad_cam_free(raw);
    let list: Value = serde_json::from_str(&text).unwrap();
    let ids: Vec<&str> = list["materials"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    for wanted in [
        "copper-c110",
        "brass-c360",
        "aluminium-6061-t6",
        "mdf",
        "fr4-copper-clad",
    ] {
        assert!(ids.contains(&wanted), "{wanted} missing from {ids:?}");
    }

    // The cut that worked: 1 mm copper, Ø2 two-flute, dial 2, ~250 mm/min.
    let out = call(
        super::vcad_cam_recommend,
        &json!({
            "material": "copper-c110",
            "op": "slot",
            "tool": { "diameter": 2.0, "flutes": 2, "kind": "flat_end_mill", "flute_length": 6.0 },
            "machine": { "class": "hobby", "spindle": "dial" },
        }),
    );
    ok(&out);
    let rec = &out["recommendation"];
    assert_eq!(
        rec["dial"],
        json!("2"),
        "a dial router has to be told which position to set, and it was dial 2 that day"
    );
    assert_eq!(out["dial_spindle"], json!(true));
    let feed = f(&rec["feed_mm_min"]);
    assert!(
        (180.0..=350.0).contains(&feed),
        "feed {feed:.0} mm/min is outside the 180-350 band around the 250 that worked"
    );
    assert!(
        (0.10..=0.30).contains(&f(&rec["stepdown_mm"])),
        "stepdown {} is outside the 0.10-0.30 band around the 0.17 that worked",
        rec["stepdown_mm"]
    );
    assert!(
        rec["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n["text"].as_str().unwrap().contains("welds")),
        "copper has to warn about welding to the cutter"
    );

    // And the checker agrees with its own recommendation.
    let checked = call(
        super::vcad_cam_check_feeds,
        &json!({
            "material": "copper-c110", "op": "slot",
            "tool": { "diameter": 2.0, "flutes": 2, "kind": "flat_end_mill", "flute_length": 6.0 },
            "machine": { "class": "hobby", "spindle": "dial" },
            "settings": { "feed": feed, "plunge": f(&rec["plunge_mm_min"]),
                          "rpm": f(&rec["rpm"]), "stepdown": f(&rec["stepdown_mm"]) },
        }),
    );
    ok(&checked);
    assert_eq!(
        checked["ok"],
        json!(true),
        "the checker rejects the numbers the recommender produced: {}",
        checked["notes"]
    );

    // Ten times the feed is not a caution.
    let hot = call(
        super::vcad_cam_check_feeds,
        &json!({
            "material": "copper-c110", "op": "slot",
            "tool": { "diameter": 2.0, "flutes": 2, "kind": "flat_end_mill", "flute_length": 6.0 },
            "machine": { "class": "hobby", "spindle": "dial" },
            "settings": { "feed": feed * 10.0, "plunge": 400.0, "rpm": 13500.0, "stepdown": 0.9 },
        }),
    );
    ok(&hot);
    assert_eq!(
        hot["ok"],
        json!(false),
        "a 10x feed at nine times the depth has to be refused: {}",
        hot["notes"]
    );
}

// ---------------------------------------------------------------------------
// 9. Gears
// ---------------------------------------------------------------------------

#[test]
fn the_milestone_planet_measures_over_pins() {
    // One 20 T module-1.0 planet, thinned 0.03 for backlash, Ø1 cutter,
    // measured over Ø1.4 pins: M = 21.073815 mm.
    let out = call(
        super::vcad_cam_gear,
        &json!({
            "gear": { "module": 1.0, "teeth": 20, "backlash_thinning": 0.03 },
            "cutter_diameter": 1.0,
            "pin_diameter": 1.4,
        }),
    );
    ok(&out);
    let m = f(&out["report"]["over_pins"]["dimension"]);
    assert!(
        (m - 21.0738).abs() < 1e-3,
        "M over Ø1.4 pins is {m:.6}, not 21.0738"
    );
    assert!(
        (f(&out["recommended_pin_diameter"]) - 1.371141).abs() < 1e-5,
        "the best-size pin is {}, not 1.371141",
        out["recommended_pin_diameter"]
    );

    // The contours the cutter needs, as point lists.
    let space = out["contours"]["tooth_space"].as_array().unwrap();
    assert!(
        space.len() > 20,
        "a tooth space is more than {} points",
        space.len()
    );
    let path = out["contours"]["tool_centre_path"].as_array().unwrap();
    assert!(!path.is_empty(), "the cutter has to have somewhere to go");
    let profile = out["contours"]["full_profile"].as_array().unwrap();
    assert!(
        profile.len() > 20 * space.len() / 2,
        "the full profile carries all twenty spaces"
    );

    // A part measured 0.05 mm over nominal has to send the cutter *into* the
    // metal, not away from it.
    let measured = call(
        super::vcad_cam_gear,
        &json!({
            "gear": { "module": 1.0, "teeth": 20, "backlash_thinning": 0.03 },
            "cutter_diameter": 1.0, "pin_diameter": 1.4,
            "measured": m + 0.05, "contours": false,
        }),
    );
    ok(&measured);
    assert!(
        f(&measured["compensation"]["detail"]["thickness_error"]) > 0.0,
        "a larger reading means thicker teeth"
    );
    assert!(
        f(&measured["compensation"]["detail"]["tool_normal_offset"]) < 0.0,
        "thicker teeth mean the cutter moves into the material"
    );

    // A planetary train reports its own mesh.
    let train = call(
        super::vcad_cam_gear,
        &json!({
            "gear": { "module": 1.0, "teeth": 20 }, "cutter_diameter": 1.0, "contours": false,
            "planetary": {
                "sun": { "module": 1.0, "teeth": 10 },
                "planet": { "module": 1.0, "teeth": 20 },
                "ring": { "module": 1.0, "teeth": 50, "internal": true },
                "planets": 2,
            },
        }),
    );
    ok(&train);
    assert_eq!(
        train["planetary"]["mesh"]["assembles"],
        json!(true),
        "10 + 50 over two planets assembles: {}",
        train["planetary"]["mesh"]
    );
}

// ---------------------------------------------------------------------------
// 10. Nothing panics, everything says why
// ---------------------------------------------------------------------------

/// Every one-string entry point, with the request that is right for it.
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
fn rubbish_is_refused_in_words_and_never_panics() {
    let rubbish = [
        "",
        "{",
        "null",
        "[]",
        "\"a string\"",
        "{\"unknown\": 1}",
        "{\"stock\": {\"thickness\": null}}",
    ];
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
        // A null pointer is not a crash either.
        let raw = entry(std::ptr::null());
        assert!(!raw.is_null(), "{name} returned null for a null request");
        let text = unsafe { CStr::from_ptr(raw) }.to_str().unwrap().to_string();
        super::vcad_cam_free(raw);
        let out: Value = serde_json::from_str(&text).unwrap();
        assert!(out["error"].is_string(), "{name}: {text}");
    }
}

#[test]
fn unusable_numbers_are_named_not_swallowed() {
    let s = stator();
    let base = stator_job(0.15, 1.0, None);

    // Non-finite and out-of-range numbers, each named in the answer.
    let cases: Vec<(Value, &str)> = vec![
        (json!({ "thickness": -1.0 }), "stock.thickness"),
        (json!({ "thickness": 0.0 }), "stock.thickness"),
    ];
    for (stock, expect) in cases {
        let mut req = base.clone();
        req["stock"] = stock;
        let out = call(super::vcad_cam_job, &req);
        let message = out["error"].as_str().expect("refused");
        assert!(message.contains(expect), "expected {expect} in {message:?}");
    }

    for (field, value, expect) in [
        ("stepdown", json!(0.0), "stepdown"),
        ("feed", json!(-250.0), "feed"),
        ("rpm", json!(0.0), "rpm"),
        ("depth", json!(0.0), "depth"),
    ] {
        let mut req = base.clone();
        req["operations"][0][field] = value;
        let out = call(super::vcad_cam_job, &req);
        let message = out["error"]
            .as_str()
            .unwrap_or_else(|| panic!("{field} was accepted: {out}"));
        assert!(message.contains(expect), "expected {expect} in {message:?}");
    }

    // NaN cannot even be written as JSON, so it arrives as a string; that is
    // still a refusal and still not a panic.
    let mut req = base.clone();
    req["operations"][0]["feed"] = json!("NaN");
    assert!(call(super::vcad_cam_job, &req)["error"].is_string());

    // An empty contour is refused by name.
    let mut req = base.clone();
    req["operations"][0]["contour"] = json!([]);
    let out = call(super::vcad_cam_job, &req);
    assert!(
        out["error"].as_str().unwrap().contains("three"),
        "{}",
        out["error"]
    );

    // A tool an operation names but the library does not hold.
    let mut req = base;
    req["operations"][0]["tool"] = json!(7);
    let out = call(super::vcad_cam_job, &req);
    assert!(
        out["error"].as_str().unwrap().contains("T7"),
        "{}",
        out["error"]
    );

    // A helical bore smaller than its cutter is refused rather than silently
    // becoming a plunge.
    let out = call(
        super::vcad_cam_job,
        &json!({
            "stock": { "thickness": 1.0, "margin": 2.0 },
            "tools": [d2_tool(1)],
            "operations": [{ "tool": 1, "kind": "helical_bore", "x": 10.0, "y": 10.0,
                             "diameter": 1.5, "depth": 1.0, "stepdown": 0.2,
                             "feed": 250, "plunge": 40, "rpm": 13500 }],
            "options": { "part": { "outer": s.outer, "holes": [] } },
        }),
    );
    assert!(
        out["error"]
            .as_str()
            .unwrap()
            .contains("larger than the cutter"),
        "{}",
        out["error"]
    );
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
// 11. The old door still opens
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
