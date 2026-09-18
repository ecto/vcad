//! The CAM JSON surface, exercised as the front ends see it.
//!
//! House rule 1 applies: these assert *geometry* — sides, diameters, depths,
//! the envelope the cutter sweeps, which checks blocked — never that a call
//! came back non-empty. The fixture is the part that was really cut on
//! 2026-09-17: the rana-60 stator, 1 mm copper, Ø2 two-flute.
//!
//! These moved here from `vcad-ffi`'s `cam/tests.rs` unchanged in behaviour
//! when the schema moved. What stayed behind there is what is genuinely about
//! the C boundary: the panic guard, the ownership contract, the null pointer,
//! the scene handle and the legacy entry point.

use serde_json::{json, Value};
use vcad_kernel_cam::outline::{read_dxf, Loop, Outline};

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Call a one-string-in, one-string-out entry point and parse the answer.
fn call(f: fn(&str) -> String, request: &Value) -> Value {
    call_text(f, &request.to_string())
}

fn call_text(f: fn(&str) -> String, request: &str) -> Value {
    let text = f(request);
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
    let out = call(super::job, &stator_job(0.15, 1.0, None));
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
    let out = call(super::job, &stator_job(0.0, 0.8, None));
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
    // …and nothing to re-post either. `moves` is the same program in machine
    // coordinates; withholding only the G-code left the door open.
    assert!(
        out.get("moves").is_none(),
        "a blocked job must not carry the moves either"
    );
    assert!(
        out["verification"].is_object(),
        "a blocked job still has to say what it found"
    );
}

/// The same refusal with no `options` at all. serde's field defaults only run
/// when the map is present; a derived `Default` on the options struct made a
/// request without one skip verification and hand back G-code as `blocked:
/// false`. Verification is on unless it is turned off by name.
#[test]
fn leaving_options_out_does_not_turn_verification_off() {
    let mut req = stator_job(0.0, 0.8, None);
    req.as_object_mut().unwrap().remove("options");
    let out = call(super::job, &req);
    ok(&out);
    assert_eq!(out["policy"]["verified"], json!(true), "{}", out["policy"]);
    assert_eq!(out["blocked"], json!(true), "{}", out["policy"]);
    assert!(out.get("gcode").is_none());
    assert!(out.get("moves").is_none());

    // And a misspelt key is refused rather than ignored: `option: { verify:
    // false }` must not be a way to get an unverified program either.
    let mut req = stator_job(0.0, 0.8, None);
    let object = req.as_object_mut().unwrap();
    object.remove("options");
    object.insert("option".into(), json!({ "verify": false }));
    let out = call(super::job, &req);
    let message = out["error"].as_str().expect("an unknown key is an error");
    assert!(message.contains("option"), "{message}");
    assert!(out.get("gcode").is_none());
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
    let bare = call(super::job, &drill(None));
    let message = bare["error"]
        .as_str()
        .expect("a break-through over nothing has to be refused");
    assert!(
        message.contains("0.300 mm past the underside") && message.contains("spoilboard"),
        "the refusal has to say how far past and what is missing: {message:?}"
    );

    // Over 3 mm of MDF: allowed, and the drill really pecks.
    let over_board = call(super::job, &drill(Some(3.0)));
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
    let out = call(super::job, &wrong_side_job(&s));
    ok(&out);
    assert_eq!(out["blocked"], json!(true), "{}", out["policy"]);
    let blocked_by: Vec<String> =
        serde_json::from_value(out["policy"]["blocked_by"].clone()).unwrap();
    assert!(
        blocked_by.iter().any(|c| c == "gouge"),
        "the refusal has to name the gouge check, got {blocked_by:?}"
    );
    assert!(out.get("gcode").is_none());
    assert!(out.get("moves").is_none());
    let worst = f(&out["verification"]["gouge"]["worst"]);
    assert!(
        (1.0..=3.5).contains(&worst),
        "a loop pushed 2 mm into the wall with a Ø2 cutter should gouge by about a tool diameter, not {worst:.3} mm"
    );
}

/// The bore loop pushed 2 mm outward by hand: the shape of item 32's mistake.
fn wrong_side_job(s: &Stator) -> Value {
    let c = centroid(&s.holes[0]);
    let wrong: Vec<[f64; 2]> = s.holes[0]
        .iter()
        .map(|p| {
            let (dx, dy) = (p[0] - c[0], p[1] - c[1]);
            let r = dx.hypot(dy).max(1e-9);
            [p[0] + 2.0 * dx / r, p[1] + 2.0 * dy / r]
        })
        .collect();
    json!({
        "name": "wrong side",
        "stock": { "thickness": 1.0, "margin": 2.0 },
        "tools": [d2_tool(1)],
        "operations": [{
            "name": "bore and slots", "tool": 1, "kind": "contour_inside",
            "contour": wrong, "depth": 1.0, "stepdown": 0.17,
            "feed": 250, "plunge": 40, "rpm": 13500, "bottom_allowance": 0.15,
        }],
        "options": { "part": { "outer": s.outer, "holes": s.holes } },
    })
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
    let out = call(super::job, &two_tool_job());
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
    let plain = call(super::job, &stator_job(0.15, 1.0, None));
    let mut with_arcs = stator_job(0.15, 1.0, None);
    with_arcs["options"]["arc_fit"] = json!({ "tolerance": 0.01 });
    let fitted = call(super::job, &with_arcs);
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
    let out = call(super::verify_gcode, &request);
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
// 6. Outline out of a mesh
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
        super::outline_from_mesh,
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
        super::compare_outline,
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
fn a_stale_outline_is_caught_before_it_machines_the_wrong_part() {
    // Item 16, in one call: the same part with one pilot missing is not the
    // same part, and nothing about its bounds says so.
    let dxf = read_dxf(&fixture("stator-outline.dxf")).unwrap();
    let mut stale = dxf.clone();
    stale.regions[0].holes.pop();
    let compare = call(
        super::compare_outline,
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

/// A cone is the shape a contour job must not be handed: every Z plane cuts a
/// different circle, so "the outline" is a question with no single answer.
fn cone(radius: f64, height: f64, segments: usize) -> (Vec<[f64; 3]>, Vec<u32>) {
    let mut positions = vec![[0.0, 0.0, height], [0.0, 0.0, 0.0]];
    for i in 0..segments {
        let a = std::f64::consts::TAU * i as f64 / segments as f64;
        positions.push([radius * a.cos(), radius * a.sin(), 0.0]);
    }
    let mut indices = Vec::new();
    for i in 0..segments {
        let (a, b) = (2 + i as u32, 2 + ((i + 1) % segments) as u32);
        indices.extend_from_slice(&[0, a, b]); // side
        indices.extend_from_slice(&[1, b, a]); // base
    }
    (positions, indices)
}

#[test]
fn a_cone_is_reported_as_not_prismatic_with_the_disagreement_measured() {
    let (positions, indices) = cone(10.0, 20.0, 96);
    let out = call(
        super::outline_from_mesh,
        &json!({ "positions": positions, "indices": indices, "auto_z": true }),
    );
    ok(&out);
    assert_eq!(
        out["prismatic"]["prismatic"],
        json!(false),
        "a cone is not constant-section: {}",
        out["prismatic"]
    );
    // And it says by how much, in millimetres a machinist can judge. The
    // reference plane is mid-height, where a Ø20 × 20 cone measures R5; the
    // worst plane sampled is the lowest, at z 1, where it measures R9.5. The
    // two disagree by 4.5 mm of wall — a quarter of the part, not a rounding.
    assert!(
        (f(&out["prismatic"]["max_boundary_distance"]) - 4.5).abs() < 0.05,
        "the worst disagreement is {}, not the 4.5 mm R9.5-against-R5 gives",
        out["prismatic"]["max_boundary_distance"]
    );
    assert!(
        (f(&out["prismatic"]["worst_z"]) - 1.0).abs() < 1e-9,
        "the widest section is the lowest one, not z {}",
        out["prismatic"]["worst_z"]
    );
    // The section at that height is still a real one: the part is a bad fit
    // for a contour job, not unreadable. Mid-height a Ø20 × 20 cone is a Ø10
    // disc, so 78.5 mm² — not a circular *hole*, so it is not in `circles`.
    assert!(
        (f(&out["area"]) - std::f64::consts::PI * 25.0).abs() < 0.5,
        "mid-height a Ø20 × 20 cone sections to {} mm², not the 78.54 a Ø10 disc gives",
        out["area"]
    );
    assert!(
        out["circles"].as_array().is_some_and(Vec::is_empty),
        "a cone has no holes, so nothing belongs in `circles`: {}",
        out["circles"]
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
        super::fit,
        &json!({ "contour": s.holes[0], "tool_diameter": 2.0, "side": "inside" }),
    );
    let large = call(
        super::fit,
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
    let list: Value = serde_json::from_str(&super::materials()).unwrap();
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
        super::recommend,
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
        super::check_feeds,
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
        super::check_feeds,
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
        super::gear,
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
        super::gear,
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
        super::gear,
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
// 9b. The claim deposit
// ---------------------------------------------------------------------------

/// Parse a deposit's `report` field back into the claim set it carries.
fn claim_set(deposit: &Value) -> vcad_kernel_cam::receipt::ClaimSet {
    let text = deposit["report"]
        .as_str()
        .unwrap_or_else(|| panic!("a deposit's report is JSON text: {deposit}"));
    serde_json::from_str(text).expect("the deposit round-trips through the registry's wire form")
}

/// The status of one named claim, e.g. `"job.no_gouge"`.
fn status(set: &vcad_kernel_cam::receipt::ClaimSet, name: &str) -> String {
    let c = set
        .claims
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| {
            panic!(
                "no claim {name} among {:?}",
                set.claims.iter().map(|c| &c.name).collect::<Vec<_>>()
            )
        });
    format!("{:?}", c.status)
}

#[test]
fn a_posted_job_claims_what_it_will_make_and_says_what_it_rests_on() {
    let out = call(super::job, &stator_job(0.15, 1.0, None));
    ok(&out);
    let deposit = &out["claims"];
    assert_eq!(deposit["schema"], json!("vcad.cam-claims/1"));

    let set = claim_set(deposit);
    // The geometric checks passed, so the claims about the *program* hold
    // outright — arithmetic needs no measurement.
    assert_eq!(status(&set, "job.no_gouge"), "Holds");
    assert_eq!(status(&set, "job.depth"), "Holds");
    assert_eq!(status(&set, "job.rapids_safe"), "Holds");
    assert_eq!(status(&set, "job.loose_pieces"), "Holds");
    // …and the claim about the *metal* does not. This is the ladder: a
    // prediction about a part nobody has cut is Provisional, never a pass.
    assert_eq!(
        status(&set, "job.tabs_hold"),
        "Provisional",
        "a tab that has never held anything cannot Hold"
    );
    // No travel limits were given, so the envelope check refuses to pass
    // vacuously rather than reporting a fit against nothing.
    assert_eq!(status(&set, "job.envelope_in_travel"), "Unverified");
    assert!(
        !set.all_hold(),
        "a set with a Provisional claim has not earned a clean verdict"
    );

    // The inputs are what make the claims re-checkable later: the program
    // that runs, the part it is checked against, the tool, the stock.
    let inputs = deposit["inputs"].as_object().expect("an inputs map");
    for key in ["program", "outline", "tool", "stock"] {
        assert!(inputs.contains_key(key), "no {key} in {inputs:?}");
    }
    // Every claim's basis key is one the deposit actually carries — a claim
    // resting on an input nobody recorded would read Stale forever.
    for claim in &set.claims {
        for key in &claim.depends_on {
            assert!(
                inputs.contains_key(key),
                "{} rests on {key}, which the deposit does not record",
                claim.name
            );
        }
    }
    // The program in the deposit is the G-code that was handed back, so
    // editing the program is what invalidates the claims.
    let program: String = serde_json::from_str(inputs["program"].as_str().unwrap()).unwrap();
    assert_eq!(program, out["gcode"].as_str().unwrap());
}

#[test]
fn a_refused_job_still_says_which_claim_it_violated() {
    // The inside contour offset the wrong way: it cuts into the part.
    let s = stator();
    let out = call(super::job, &wrong_side_job(&s));
    ok(&out);
    assert_eq!(out["blocked"], json!(true), "this job gouges");
    // No G-code, and a claim set that says exactly why — a receipt that only
    // ever saw passing jobs would be a record of nothing.
    assert!(out.get("gcode").is_none() || out["gcode"].is_null());
    assert!(out.get("moves").is_none());
    let set = claim_set(&out["claims"]);
    assert_eq!(status(&set, "job.no_gouge"), "Violated");
    let gouge = set.find("job.no_gouge", None).unwrap();
    assert!(
        gouge.value.unwrap_or(0.0) > 0.0,
        "a violated gouge claim carries how far in it cut"
    );
    assert!(
        gouge
            .detail
            .as_deref()
            .unwrap_or("")
            .contains("enter the part"),
        "and says so in words: {:?}",
        gouge.detail
    );
}

#[test]
fn a_gear_deposits_the_dimension_it_predicts_and_the_gear_to_correct_it_with() {
    let out = call(
        super::gear,
        &json!({
            "gear": { "module": 1.0, "teeth": 20, "backlash_thinning": 0.03 },
            "cutter_diameter": 1.0, "pin_diameter": 1.4, "contours": false,
        }),
    );
    ok(&out);
    let set = claim_set(&out["claims"]);
    let subject = out["claim_subject"].as_str().expect("a filing subject");
    assert_eq!(subject, "20T-m1");

    let pins = set
        .find("gear.over_pins", Some(subject))
        .expect("the over-pins claim is filed under the gear's subject");
    assert_eq!(format!("{:?}", pins.status), "Provisional");
    // The claim carries the dimension the report predicts, to the micron.
    let predicted = pins.value.expect("a predicted dimension");
    assert!(
        (predicted - f(&out["report"]["over_pins"]["dimension"])).abs() < 1e-9,
        "the claim and the report must predict the same M"
    );
    assert!((predicted - 21.0738).abs() < 1e-3, "M is {predicted:.6}");

    // The gear itself rides in the inputs: without it a later measurement
    // can say the teeth are fat and not by how much to move the cutter.
    let inputs = out["claims"]["inputs"].as_object().unwrap();
    let gear: Value = serde_json::from_str(inputs["gear"].as_str().unwrap()).unwrap();
    assert_eq!(f(&gear["module"]), 1.0);
    assert_eq!(gear["teeth"], json!(20));

    // `program` is recorded as absent, not left out. A measurement adds
    // `gear.over_pins.compensated`, which rests on the program that will cut
    // the next part; a basis key the fingerprint does not carry reads as
    // changed, so leaving it out would make that claim permanently Stale
    // instead of the Provisional prediction it is.
    assert_eq!(
        inputs["program"].as_str(),
        Some("null"),
        "a gear job has no program yet, and says so rather than staying silent"
    );

    // Every claim's basis key — including the one only a measurement adds —
    // is a key this deposit carries.
    for claim in &set.claims {
        for key in &claim.depends_on {
            assert!(
                inputs.contains_key(key),
                "{} rests on {key}, which the deposit does not record",
                claim.name
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 10. Tabs where you put them
// ---------------------------------------------------------------------------

#[test]
fn explicit_tab_positions_land_where_they_were_asked_for_and_the_audit_says_where() {
    // Six tabs crowded into the first 42% of the loop. The kernel settles each
    // one onto the nearest straight stretch, but never further than half a tab
    // pitch — one twelfth of the loop here — so every one of them has to end
    // up in the *first half*. Even spacing puts three of six in the second
    // half, so this is a result no default could produce.
    let asked = [0.02, 0.10, 0.18, 0.26, 0.34, 0.42];
    let mut request = stator_job(0.15, 1.0, None);
    let profile = request["operations"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    assert_eq!(profile["name"], json!("profile"));
    profile["tab_positions"] = json!(asked);
    profile.as_object_mut().unwrap().remove("tabs");

    let out = call(super::job, &request);
    ok(&out);
    assert_eq!(out["blocked"], json!(false), "{}", out["policy"]);
    assert_eq!(
        out["verification"]["tabs"]["tab_count"],
        json!(6),
        "six tabs were asked for and six have to stand: {}",
        out["verification"]["tabs"]["check"]
    );

    let audit = &out["tab_placement"][0];
    assert_eq!(audit["op"], json!("profile"));
    assert_eq!(audit["requested"], json!(6));
    assert_eq!(
        audit["found"],
        json!(6),
        "six tabs were asked for and six lifted stretches have to be in the toolpath: {audit}"
    );
    assert_eq!(audit["requested_positions"], json!(asked));

    let tabs = audit["tabs"].as_array().unwrap();
    let perimeter = f(&audit["perimeter_mm"]);
    assert!(
        (perimeter - 215.0).abs() < 20.0,
        "the stator's outline is about 215 mm round, and this reads {perimeter:.1}"
    );
    for tab in tabs {
        let width = f(&tab["metal_width"]);
        assert!(
            (width - 4.0).abs() < 0.1,
            "a 4 mm tab left {width:.3} mm of metal standing"
        );
        // A tab is measured from the *underside* of the stock, not from the
        // floor the cut reaches: a 0.42 mm tab under a 0.15 mm onion skin
        // stands 0.27 mm above the -0.85 the cutter got to.
        assert!(
            (f(&tab["height"]) - 0.27).abs() < 0.03,
            "a 0.42 mm tab over a 0.15 mm skin stands {} mm above the floor, not 0.27",
            tab["height"]
        );
        assert_eq!(
            tab["straight"],
            json!(true),
            "settling exists to put tabs on straight wall, and this one is not: {tab}"
        );
    }

    // The decisive geometry: asking for six tabs inside 40% of the loop has to
    // *crowd* them. Five of the six gaps are short and one is the long way
    // back round — even spacing gives six equal gaps of a sixth of the loop.
    let mut gaps: Vec<f64> = tabs.iter().map(|t| f(&t["gap_to_next_mm"])).collect();
    assert_eq!(
        gaps.len(),
        6,
        "six tabs make six gaps round a closed loop: {gaps:?}"
    );
    assert!(
        (gaps.iter().sum::<f64>() - perimeter).abs() < 1e-6,
        "the gaps between six tabs have to add up to the loop: {gaps:?}"
    );
    gaps.sort_by(f64::total_cmp);
    let even = perimeter / 6.0;
    assert!(
        gaps[5] > 2.0 * even,
        "crowding six tabs into 40% of the loop has to leave one gap far longer than the {even:.1} mm even spacing gives, and the longest is {:.1}",
        gaps[5]
    );
    assert!(
        gaps[4] < even,
        "the other five gaps have to be shorter than even spacing, and the widest of them is {:.1}",
        gaps[4]
    );
}

#[test]
fn evenly_spaced_tabs_come_out_evenly_spaced() {
    // The default spacing, which the real job used: three tabs, a third of the
    // loop apart. Settling moves each one onto straight wall, so the gaps are
    // near-equal rather than exactly equal — and the audit measures them
    // rather than assuming them.
    let out = call(super::job, &stator_job(0.15, 1.0, None));
    ok(&out);
    let audit = &out["tab_placement"][0];
    let tabs = audit["tabs"].as_array().unwrap();
    assert_eq!(tabs.len(), 3, "{audit}");
    let perimeter = f(&audit["perimeter_mm"]);
    let even = perimeter / 3.0;
    for tab in tabs {
        let gap = f(&tab["gap_to_next_mm"]);
        assert!(
            (gap - even).abs() < 0.1 * even,
            "three evenly-spaced tabs sit {even:.1} mm apart, and this one is {gap:.1} mm from the next"
        );
        assert_eq!(tab["straight"], json!(true));
        assert_eq!(
            tab["passes"],
            json!(2),
            "the profile reaches tab height on two passes, so each tab is seen twice: {tab}"
        );
    }
}

#[test]
fn a_tab_count_that_disagrees_with_its_positions_is_refused() {
    let mut request = stator_job(0.15, 1.0, None);
    let profile = request["operations"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    profile["tab_positions"] = json!([0.1, 0.5]);
    let out = call(super::job, &request);
    let message = out["error"]
        .as_str()
        .expect("3 tabs and 2 positions cannot both be true");
    assert!(
        message.contains("3 tabs") && message.contains("2 position"),
        "the refusal has to say which two numbers disagree: {message:?}"
    );

    // And a position off the loop is named rather than wrapped.
    let profile = request["operations"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap();
    profile["tab_positions"] = json!([0.1, 0.5, 1.7]);
    let out = call(super::job, &request);
    let message = out["error"].as_str().expect("1.7 is not a fraction");
    assert!(
        message.contains("tab_positions[2]") && message.contains("1.7"),
        "{message:?}"
    );
}

// ---------------------------------------------------------------------------
// 11. Where the part sits on the stock
// ---------------------------------------------------------------------------

/// The stator job, placed on the stock.
///
/// `job_level` puts the placement in `options`, where it moves the part the
/// oracle checks against too; otherwise it goes on every operation, which
/// moves the cuts and leaves the part where it was drawn.
fn placed_stator(rotation_deg: f64, dx: f64, dy: f64, job_level: bool) -> Value {
    let placement = json!({ "dx": dx, "dy": dy, "rotation_deg": rotation_deg });
    let mut request = stator_job(0.15, 1.0, None);
    if job_level {
        request["options"]["placement"] = placement;
    } else {
        for op in request["operations"].as_array_mut().unwrap() {
            op["placement"] = placement.clone();
        }
    }
    request
}

#[test]
fn a_placed_job_cuts_the_placed_part_and_verifies_against_it() {
    // 30° about the stock origin and 40 mm along X: a placement no rounding
    // could produce by accident.
    let out = call(super::job, &placed_stator(30.0, 40.0, 5.0, true));
    ok(&out);
    assert_eq!(
        out["blocked"],
        json!(false),
        "a job and the part it is checked against moved together and still reads as wrong: {}",
        out["policy"]
    );
    assert_eq!(out["verification"]["pass"], json!(true));

    // The part really moved *there*: the far corner of the sweep is the far
    // corner of the turned-and-shifted outline, plus the radius the outside
    // profile stands off and the radius the cutter sweeps. The unplaced job
    // reached 65.145; this one has to reach the number the placement predicts,
    // worked out from the fixture rather than assumed.
    let turned = crate::Placement::new(40.0, 5.0, 30.0).apply_loop(&stator().outer);
    let env = &out["verification"]["envelope"];
    for axis in 0..2 {
        let far = turned.iter().map(|p| p[axis]).fold(f64::MIN, f64::max) + 2.0;
        assert!(
            (f(&env["work_max"][axis]) - far).abs() < 0.05,
            "on axis {axis} the sweep reaches {}, and the placed outline says {far:.3}",
            env["work_max"][axis]
        );
    }
    // A turn is not a shift: the placed part is nowhere near where it was
    // drawn, on both axes.
    assert!(
        f(&env["work_max"][0]) > 80.0 && f(&env["work_max"][1]) > 85.0,
        "the placed job still ends at {} / {}, so nothing turned",
        env["work_max"][0],
        env["work_max"][1]
    );
}

#[test]
fn a_placed_job_checked_against_the_unplaced_part_is_refused() {
    // The mistake this field exists to make impossible: the cuts moved, the
    // part did not. A per-operation placement does exactly that, and the
    // oracle has to notice rather than machining air beside the stock.
    let out = call(super::job, &placed_stator(30.0, 40.0, 5.0, false));
    ok(&out);
    assert_eq!(
        out["blocked"],
        json!(true),
        "cutting a rotated part against an unrotated outline has to be refused: {}",
        out["policy"]
    );
    assert!(
        out.get("gcode").is_none(),
        "a blocked job must not carry a gcode key at all"
    );
    assert!(
        out.get("moves").is_none(),
        "a blocked job must not carry the moves either"
    );
    let blocked_by: Vec<String> =
        serde_json::from_value(out["policy"]["blocked_by"].clone()).unwrap();
    assert!(
        blocked_by.iter().any(|c| c == "gouge"),
        "the cuts land in the middle of the part as drawn, which is a gouge: {blocked_by:?}"
    );
    // And it said so before the run, not after: the caution names the
    // operation whose placement did not take the part with it.
    let notes = out["notes"].as_array().unwrap();
    assert!(
        notes.iter().any(|n| n["text"]
            .as_str()
            .unwrap_or("")
            .contains("its own placement")),
        "an operation that moved alone has to be called out: {notes:?}"
    );
}

#[test]
fn a_placement_that_moves_nothing_changes_nothing() {
    // The zero case is worth pinning: an identity placement must not perturb
    // the geometry by so much as a rounding step, or every job that carries a
    // placement field would drift.
    let plain = call(super::job, &stator_job(0.15, 1.0, None));
    let placed = call(super::job, &placed_stator(0.0, 0.0, 0.0, true));
    ok(&plain);
    ok(&placed);
    assert_eq!(
        plain["gcode"], placed["gcode"],
        "an identity placement rewrote the program"
    );
}

#[test]
fn a_turned_rectangle_is_refused_rather_than_quietly_boxed() {
    // A face is stated as `[x0, y0, x1, y1]`, which cannot survive a turn.
    // Taking its bounding box would face a larger area than was asked for.
    let request = json!({
        "name": "face the blank",
        "stock": { "thickness": 6.0, "bbox": [0.0, 0.0, 80.0, 50.0] },
        "tools": [d2_tool(1)],
        "operations": [{
            "name": "facing", "tool": 1, "kind": "face",
            "rectangle": [0.0, 0.0, 80.0, 50.0], "depth": 0.5, "stepdown": 0.25,
            "feed": 600, "plunge": 100, "rpm": 12000,
        }],
        "options": { "verify": false, "placement": { "rotation_deg": 12.0 } },
    });
    let out = call(super::job, &request);
    let message = out["error"]
        .as_str()
        .expect("a turned rectangle is refused");
    assert!(
        message.contains("rectangle") && message.contains("contour"),
        "the refusal has to say what to do instead: {message:?}"
    );

    // A pure shift is fine, though, and shifts by exactly what it says. A
    // facing pass overhangs the blank by a radius, so the absolute number is
    // the kernel's business; the *difference* is the placement's, and it has
    // to be the 3 mm asked for to the last digit.
    let left_edge = |placement: Option<Value>| -> f64 {
        let mut req = request.clone();
        match placement {
            Some(p) => req["options"]["placement"] = p,
            None => {
                req["options"].as_object_mut().unwrap().remove("placement");
            }
        }
        let out = call(super::job, &req);
        ok(&out);
        out["moves"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| f(&m["to"][0]))
            .fold(f64::MAX, f64::min)
    };
    let plain = left_edge(None);
    let shifted = left_edge(Some(json!({ "dx": 3.0, "dy": -1.0 })));
    assert!(
        (shifted - plain - 3.0).abs() < 1e-9,
        "a 3 mm shift moved the facing pass from {plain:.6} to {shifted:.6}"
    );
}

// ---------------------------------------------------------------------------
// 11b. Islands: material a pocket keeps
// ---------------------------------------------------------------------------

/// The run order of the operation blocks, by name.
fn run_order(out: &Value) -> Vec<String> {
    let mut blocks: Vec<(u64, String)> = out["op_ranges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|r| r["block"] == json!("operation"))
        .map(|r| {
            (
                r["start"].as_u64().unwrap(),
                r["name"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    blocks.sort_by_key(|(start, _)| *start);
    blocks.into_iter().map(|(_, name)| name).collect()
}

/// A circle as a closed loop, the way an outline hands one over.
fn circle_loop(centre: [f64; 2], radius: f64, segments: usize) -> Vec<[f64; 2]> {
    (0..segments)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / segments as f64;
            [centre[0] + radius * a.cos(), centre[1] + radius * a.sin()]
        })
        .collect()
}

/// Where the two kept bosses stand, and how big they are.
const BOSSES: [[f64; 2]; 2] = [[20.0, 20.0], [40.0, 20.0]];
const BOSS_RADIUS: f64 = 4.0;

/// A plate with a rectangular pocket and two bosses left standing in it.
///
/// The stator's own opening cannot carry its pilots as islands — the three
/// M3 pilots are outside that loop, in the ring, not inside it — and the
/// kernel says so (see `islands_belong_to_pockets_and_travel_with_them`). So
/// the geometry here is the plainest shape that asks the real question: metal
/// the pocket must clear *around* and leave standing.
fn pocket_with_islands(islands: bool, dx: f64) -> Value {
    let wall = vec![
        [10.0 + dx, 10.0],
        [50.0 + dx, 10.0],
        [50.0 + dx, 30.0],
        [10.0 + dx, 30.0],
    ];
    let mut pocket = json!({
        "name": "clear around the bosses", "tool": 1, "kind": "pocket",
        "contour": wall, "depth": 2.0, "stepdown": 0.5, "stepover": 0.8,
        "feed": 400, "plunge": 100, "rpm": 12000,
    });
    if islands {
        pocket["islands"] = json!(BOSSES
            .iter()
            .map(|c| circle_loop([c[0] + dx, c[1]], BOSS_RADIUS, 48))
            .collect::<Vec<_>>());
    }
    json!({
        "name": "bossed plate",
        "stock": { "thickness": 6.0, "bbox": [0.0, 0.0, 60.0, 40.0] },
        "machine": { "name": "Anolex Ultra 2", "spindle": "dial" },
        "tools": [d2_tool(1)],
        "operations": [pocket],
        // A blind pocket is a 2 mm step in a 6 mm plate, and the oracle's part
        // is a *plan*: an outer boundary and openings, with no language for a
        // floor half way down. Stating this pocket as an opening would have it
        // refused for not cutting through; stating it as solid part would have
        // every pass read as a gouge. So the replay is off here, on purpose —
        // and the island check still runs, which is the whole reason it does
        // not live behind `verify`.
        "options": { "verify": false },
    })
}

/// The closest a cutting move came to a point, in XY.
fn nearest_cut(out: &Value, to: [f64; 2]) -> f64 {
    out["moves"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["rapid"] != json!(true) && f(&m["to"][2]) <= 1e-9)
        .map(|m| (f(&m["to"][0]) - to[0]).hypot(f(&m["to"][1]) - to[1]))
        .fold(f64::MAX, f64::min)
}

#[test]
fn a_pocket_clears_around_its_islands_and_never_touches_them() {
    // A Ø2 cutter clearing around a Ø8 boss may bring its centre no closer
    // than 4 + 1 mm to the boss's centre.
    let keep_out = BOSS_RADIUS + 1.0;
    let out = call(super::job, &pocket_with_islands(true, 0.0));
    ok(&out);
    assert_eq!(
        out["blocked"],
        json!(false),
        "clearing around two bosses is a legal job: {} {}",
        out["policy"],
        out["error"]
    );

    // Measured off the moves, not off the report that claims it.
    for (i, boss) in BOSSES.iter().enumerate() {
        let nearest = nearest_cut(&out, *boss);
        assert!(
            nearest >= keep_out - 0.03,
            "boss {i} is kept material and the cutter centre came {nearest:.3} mm from it \
             — under {keep_out:.3} mm it has taken metal out of it"
        );
    }

    // And the report's own number is that measurement, not a restatement of
    // the request: centre clearance is the distance to the wall, so it has to
    // agree with the measured distance to the centre, less the boss radius.
    let audit = &out["island_clearance"][0];
    assert_eq!(audit["op"], json!("clear around the bosses"));
    let islands = audit["islands"].as_array().unwrap();
    assert_eq!(islands.len(), 2, "two bosses, two islands");
    let tolerance = f(&audit["tolerance_mm"]);
    for (i, island) in islands.iter().enumerate() {
        assert_eq!(island["kept"], json!(true), "{island}");
        // Nothing beyond the measurement's own slack: the preview samples arcs
        // to a chord tolerance, and a 48-sided island is itself a chord
        // approximation, so a micron either way is the instrument, not a cut.
        assert!(
            f(&island["cut_into_mm"]) < tolerance,
            "nothing may be cut out of an island: {island}"
        );
        let measured = nearest_cut(&out, BOSSES[i]) - BOSS_RADIUS;
        assert!(
            (f(&island["centre_clearance_mm"]) - measured).abs() < 0.02,
            "the report says the cutter centre stayed {} mm off island {i}'s wall, and the \
             moves say {measured:.3} mm",
            island["centre_clearance_mm"]
        );
    }

    // The mutation this test exists to catch: drop the islands from the
    // request and the pocket clears both bosses away.
    let plain = call(super::job, &pocket_with_islands(false, 0.0));
    ok(&plain);
    let worst = BOSSES
        .iter()
        .map(|boss| nearest_cut(&plain, *boss))
        .fold(f64::MAX, f64::min);
    assert!(
        worst < BOSS_RADIUS,
        "without islands the pocket has to cut the bosses away (it came {worst:.3} mm from a \
         centre), or this test would pass whether islands work or not"
    );
    assert!(
        plain.get("island_clearance").is_none(),
        "no islands, no island report"
    );
}

#[test]
fn islands_belong_to_pockets_and_travel_with_them() {
    // An island on a contour is a different thing — a second wall — so it is
    // refused rather than quietly ignored.
    let s = stator();
    let mut request = stator_job(0.15, 1.0, None);
    request["operations"][0]["islands"] = json!([s.holes[1]]);
    let out = call(super::job, &request);
    let message = out["error"].as_str().expect("refused");
    assert!(
        message.contains("pocket"),
        "the refusal has to name what does take islands: {message:?}"
    );

    // An island that is not inside the pocket is refused too, in the kernel's
    // own words. This is what the stator asks: its three pilots sit in the
    // ring, outside the bore-and-slots opening, so they cannot be that
    // pocket's islands however sensible the sentence sounds.
    let mut request = stator_job(0.15, 1.0, None);
    request["operations"][0]["kind"] = json!("pocket");
    request["operations"][0]["islands"] = json!(s.holes[1..].to_vec());
    let out = call(super::job, &request);
    let message = out["error"].as_str().expect("refused");
    assert!(
        message.contains("island") && message.contains("outside"),
        "an island outside its pocket is refused, not clipped: {message:?}"
    );

    // And a placed pocket takes its islands with it: the kept lump moves with
    // the metal, or the cutter clears around where it used to be.
    let mut req = pocket_with_islands(true, 0.0);
    req["options"]["placement"] = json!({ "dx": 5.0, "dy": 0.0 });
    let out = call(super::job, &req);
    ok(&out);
    let moved = [BOSSES[0][0] + 5.0, BOSSES[0][1]];
    assert!(
        nearest_cut(&out, moved) >= BOSS_RADIUS + 1.0 - 0.03,
        "the island moved with the pocket, so the cutter has to keep off it there: {:.3} mm",
        nearest_cut(&out, moved)
    );
    assert!(
        nearest_cut(&out, BOSSES[0]) < BOSS_RADIUS,
        "and the metal where the boss used to be is cleared, or nothing moved at all"
    );
    assert!(
        f(&out["island_clearance"][0]["islands"][0]["cut_into_mm"])
            < f(&out["island_clearance"][0]["tolerance_mm"]),
        "nothing was cut out of the island it moved to: {}",
        out["island_clearance"][0]["islands"][0]
    );
}

// ---------------------------------------------------------------------------
// 11c. Phase: saying when an operation runs, without lying about what it is
// ---------------------------------------------------------------------------

#[test]
fn a_phase_moves_an_operation_without_renaming_the_cut() {
    // By default the profile that frees the part runs last, whatever order the
    // operations arrive in. That rule is right nearly always, and it is why a
    // phase has to be said out loud to change it.
    let plain = call(super::job, &stator_job(0.15, 1.0, None));
    ok(&plain);
    assert_eq!(
        run_order(&plain).last().map(String::as_str),
        Some("profile"),
        "by default the profile runs last: {:?}",
        run_order(&plain)
    );

    let mut request = stator_job(0.15, 1.0, None);
    let ops = request["operations"].as_array_mut().unwrap();
    let last = ops.len() - 1;
    ops[last]["phase"] = json!(-1);
    let out = call(super::job, &request);
    ok(&out);
    let order = run_order(&out);
    assert_eq!(
        order.first().map(String::as_str),
        Some("profile"),
        "phase −1 puts the profile first: {order:?}"
    );
    // It is still an outside profile: the cut did not change, only when it
    // runs. Same tabs, same side, same depth as the default run.
    assert_eq!(
        out["verification"]["tabs"]["tab_count"], plain["verification"]["tabs"]["tab_count"],
        "the profile is the same cut wherever it runs"
    );
    assert_eq!(
        out["verification"]["depth"]["deepest_z"],
        plain["verification"]["depth"]["deepest_z"]
    );
    // …and the job says the order came from the phases, so nobody has to guess
    // why the profile ran first.
    let notes = out["notes"].to_string();
    assert!(
        notes.contains("phase order"),
        "the notes have to say the phases decided the order: {notes}"
    );

    // Phases order everything, so equal phases keep the order they were given.
    let mut even = stator_job(0.15, 1.0, None);
    for op in even["operations"].as_array_mut().unwrap() {
        op["phase"] = json!(0);
    }
    let out = call(super::job, &even);
    ok(&out);
    assert_eq!(
        run_order(&out),
        vec!["bore and slots", "pilot 1", "pilot 2", "pilot 3", "profile"],
        "one phase for everything is the order they were written in"
    );
}

#[test]
fn a_phase_and_a_role_are_not_both_given() {
    let mut request = stator_job(0.15, 1.0, None);
    let ops = request["operations"].as_array_mut().unwrap();
    let last = ops.len() - 1;
    ops[last]["phase"] = json!(0);
    ops[last]["role"] = json!("inside_feature");
    let out = call(super::job, &request);
    let message = out["error"].as_str().expect("refused");
    assert!(
        message.contains("role") && message.contains("phase"),
        "the refusal has to name both: {message:?}"
    );
}

#[test]
fn a_phase_order_two_tools_cannot_run_is_refused_rather_than_resorted() {
    // Tool changes are grouped, so "T2, then T1, then T2 again" cannot be
    // honoured — the job would have to stop and re-probe twice. Running it in
    // some other order instead is the silent wrong answer this field exists to
    // replace, so it is refused.
    let mut request = two_tool_job();
    // The fixture is [profile T1, bore T2, pilot 1 T2, pilot 2 T2, pilot 3 T2].
    let asked = [1, 2, 0, 3, 4];
    for (op, phase) in request["operations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(asked)
    {
        op["phase"] = json!(phase);
    }
    let out = call(super::job, &request);
    let message = out["error"].as_str().unwrap_or_else(|| {
        panic!(
            "interleaved tools have to be refused, got {}",
            out["policy"]
        )
    });
    assert!(
        message.contains("phases ask for") && message.contains("tool"),
        "the refusal has to say what was asked, what would happen, and why: {message:?}"
    );

    // The same job with each tool's work kept together runs, and runs in the
    // order it was given.
    let agreeable = [4, 0, 1, 2, 3];
    for (op, phase) in request["operations"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(agreeable)
    {
        op["phase"] = json!(phase);
    }
    let out = call(super::job, &request);
    ok(&out);
    assert_eq!(
        run_order(&out),
        vec!["bore and slots", "pilot 1", "pilot 2", "pilot 3", "profile"],
        "phases the tools can honour run exactly as asked"
    );
}

// ---------------------------------------------------------------------------
// 12. Nothing panics, everything says why
// ---------------------------------------------------------------------------

/// A one-string-in, one-string-out entry point, named.
type Entry = (&'static str, fn(&str) -> String);

/// Every one-string entry point, with the request that is right for it.
fn entry_points() -> Vec<Entry> {
    vec![
        ("job", super::job as fn(&str) -> String),
        ("verify_gcode", super::verify_gcode),
        ("fit", super::fit),
        ("outline_from_mesh", super::outline_from_mesh),
        ("compare_outline", super::compare_outline),
        ("recommend", super::recommend),
        ("check_feeds", super::check_feeds),
        ("gear", super::gear),
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
        let out = call(super::job, &req);
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
        let out = call(super::job, &req);
        let message = out["error"]
            .as_str()
            .unwrap_or_else(|| panic!("{field} was accepted: {out}"));
        assert!(message.contains(expect), "expected {expect} in {message:?}");
    }

    // NaN cannot even be written as JSON, so it arrives as a string; that is
    // still a refusal and still not a panic.
    let mut req = base.clone();
    req["operations"][0]["feed"] = json!("NaN");
    assert!(call(super::job, &req)["error"].is_string());

    // An empty contour is refused by name.
    let mut req = base.clone();
    req["operations"][0]["contour"] = json!([]);
    let out = call(super::job, &req);
    assert!(
        out["error"].as_str().unwrap().contains("three"),
        "{}",
        out["error"]
    );

    // A tool an operation names but the library does not hold.
    let mut req = base;
    req["operations"][0]["tool"] = json!(7);
    let out = call(super::job, &req);
    assert!(
        out["error"].as_str().unwrap().contains("T7"),
        "{}",
        out["error"]
    );

    // A helical bore smaller than its cutter is refused rather than silently
    // becoming a plunge.
    let out = call(
        super::job,
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

// ---------------------------------------------------------------------------
// The measured numbers this file's thresholds came from
// ---------------------------------------------------------------------------

#[test]
#[ignore = "prints the measured numbers; not an assertion"]
fn zz_numbers() {
    let plain = call(super::job, &stator_job(0.15, 1.0, None));
    let mut a = stator_job(0.15, 1.0, None);
    a["options"]["arc_fit"] = json!({ "tolerance": 0.01 });
    let fitted = call(super::job, &a);
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
    println!("TABS {}", plain["tab_placement"]);

    let s = stator();
    let wrong = call(super::job, &wrong_side_job(&s));
    println!("GOUGE worst {}", wrong["verification"]["gouge"]["worst"]);
    for d in [2.0, 3.175] {
        let r = call(
            super::fit,
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
    let two = call(super::job, &two_tool_job());
    println!(
        "TWOTOOL moves {} ranges {} policy {}",
        two["moves"].as_array().unwrap().len(),
        two["op_ranges"].as_array().unwrap().len(),
        two["policy"]
    );
    let vg = call(
        super::verify_gcode,
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
        super::outline_from_mesh,
        &json!({ "positions": positions, "indices": indices, "auto_z": true }),
    );
    let cmp = call(
        super::compare_outline,
        &json!({ "dxf": fixture("stator-outline.dxf"), "outline": sec["outline"], "tolerance": 0.02 }),
    );
    println!(
        "SECTION holes {} circles {} maxdist {} symdiff {}",
        sec["regions"][0]["holes"].as_array().unwrap().len(),
        sec["circles"],
        cmp["diff"]["max_boundary_distance"],
        cmp["diff"]["symmetric_difference_area"]
    );
    let (positions, indices) = cone(10.0, 20.0, 96);
    let cone = call(
        super::outline_from_mesh,
        &json!({ "positions": positions, "indices": indices, "auto_z": true }),
    );
    println!("CONE prismatic {}", cone["prismatic"]);
    for (label, request) in [
        ("job-level", placed_stator(30.0, 40.0, 5.0, true)),
        ("op-level", placed_stator(30.0, 40.0, 5.0, false)),
    ] {
        let out = call(super::job, &request);
        println!(
            "PLACED {label} blocked {} policy {} envelope {} .. {}",
            out["blocked"],
            out["policy"],
            out["verification"]["envelope"]["work_min"],
            out["verification"]["envelope"]["work_max"]
        );
    }
}
