//! Bounded CAM jobs for the native CNC workspace: rectangular face / pocket /
//! profile, plus contour profiles (outside or inside) along an imported
//! closed polyline, with optional holding tabs.
//!
//! This is the *first* CAM surface — one operation, one tool, a header
//! assembled here by hand — and the native app still calls it. It is kept
//! working unchanged while wave 3 moves over to [`super::job`], which posts a
//! whole multi-tool job through `vcad_kernel_cam::Job` and refuses to hand
//! back G-code the verification oracle rejects.
use serde::{Deserialize, Serialize};
use vcad_kernel_cam::{
    CamSettings, Contour, Contour2D, Face, Pocket2D, Point2D, Tool, ToolpathSegment,
};

#[derive(Deserialize)]
struct Request {
    operation: String,
    width: f64,
    height: f64,
    depth: f64,
    diameter: f64,
    stepdown: f64,
    stepover: f64,
    feed: f64,
    plunge: f64,
    rpm: f64,
    clearance: f64,
    /// Closed polyline (mm, stock frame: XY lower-left at 0) for the contour
    /// operations. Ignored by the rectangular ones.
    #[serde(default)]
    contour: Vec<[f64; 2]>,
    /// Holding tabs left on an outside contour (0 = none).
    #[serde(default)]
    tabs: u32,
    #[serde(default, rename = "tabWidth")]
    tab_width: f64,
    #[serde(default, rename = "tabHeight")]
    tab_height: f64,
}

/// Build a kernel contour from a closed polyline in the stock frame.
fn polyline_contour(r: &Request) -> Result<Contour, String> {
    if r.contour.len() < 3 {
        return Err("A contour needs at least three points".into());
    }
    let margin = 0.001;
    for p in &r.contour {
        if !p[0].is_finite() || !p[1].is_finite() {
            return Err("Contour points must be finite".into());
        }
        if p[0] < -margin || p[0] > r.width + margin || p[1] < -margin || p[1] > r.height + margin {
            return Err("Contour points must lie inside the width × height region".into());
        }
    }
    let mut c = Contour::new(Point2D::new(r.contour[0][0], r.contour[0][1]));
    for p in r.contour.iter().skip(1) {
        c.line_to(Point2D::new(p[0], p[1]));
    }
    let first = r.contour[0];
    let last = r.contour[r.contour.len() - 1];
    if (first[0] - last[0]).hypot(first[1] - last[1]) > 1e-6 {
        c.line_to(Point2D::new(first[0], first[1]));
    }
    Ok(c)
}

#[derive(Serialize)]
struct Move {
    to: [f64; 3],
    rapid: bool,
    feed: f64,
}

pub(super) fn generate(input: &str) -> Result<String, String> {
    let r: Request = serde_json::from_str(input).map_err(|e| e.to_string())?;
    let values = [
        r.width,
        r.height,
        r.depth,
        r.diameter,
        r.stepdown,
        r.stepover,
        r.feed,
        r.plunge,
        r.rpm,
        r.clearance,
    ];
    if values.iter().any(|v| !v.is_finite() || *v <= 0.0)
        || r.width < 0.01
        || r.height < 0.01
        || r.depth < 0.001
        || r.diameter < 0.01
        || r.stepdown < 0.001
        || r.stepover < 0.001
        || r.feed < 0.1
        || r.plunge < 0.1
        || r.rpm < 1.0
        || r.clearance < 0.001
        || r.width > 1000.0
        || r.height > 1000.0
        || r.depth > 100.0
        || r.diameter > r.width.min(r.height)
        || r.stepover > r.diameter
        || r.clearance > 100.0
        || r.feed > 30000.0
        || r.plunge > 30000.0
        || r.rpm > 100000.0
    {
        return Err("Use finite positive dimensions, feeds and RPM; stepover must not exceed tool diameter.".into());
    }
    // Bound work before calling algorithms whose pass counts depend on input.
    if (r.depth / r.stepdown).ceil() * ((r.width.max(r.height) + r.diameter) / r.stepover).ceil()
        > 10000.0
    {
        return Err("Too many passes; increase stepdown or stepover.".into());
    }
    let tool = Tool::FlatEndMill {
        diameter: r.diameter,
        flute_length: r.depth,
        flutes: 2,
    };
    let settings = CamSettings {
        stepover: r.stepover,
        stepdown: r.stepdown,
        feed_rate: r.feed,
        plunge_rate: r.plunge,
        spindle_rpm: r.rpm,
        safe_z: r.clearance,
        retract_z: r.clearance,
    };
    let path = match r.operation.as_str() {
        "face" => Face::from_size(r.width, r.height, r.depth).generate(&tool, &settings),
        "pocket" => {
            Pocket2D::rectangle(0.0, 0.0, r.width, r.height, r.depth).generate(&tool, &settings)
        }
        "profile" => Contour2D::outside(Contour::rectangle(0.0, 0.0, r.width, r.height), r.depth)
            .generate(&tool, &settings),
        "contour_outside" | "contour_inside" => {
            let contour = polyline_contour(&r)?;
            let mut op = if r.operation == "contour_outside" {
                Contour2D::outside(contour, r.depth)
            } else {
                Contour2D::inside(contour, r.depth)
            };
            if r.tabs > 0 {
                if !r.tab_width.is_finite()
                    || !r.tab_height.is_finite()
                    || r.tab_width <= 0.0
                    || r.tab_height <= 0.0
                    || r.tab_height > r.depth
                    || r.tabs > 64
                {
                    return Err(
                        "Tabs need a positive width and a height no deeper than the cut".into(),
                    );
                }
                op = op.with_tabs(r.tabs as usize, r.tab_width, r.tab_height);
            }
            op.generate(&tool, &settings)
        }
        _ => return Err("Unknown operation".into()),
    }
    .map_err(|e| e.to_string())?;
    // Explicit coordinates avoid modal-state ambiguity after the header retract.
    // No M6: this is a single-tool job for stock Grbl_ESP32.
    let mut code = format!("(vcad single-tool {} - G54 stock top Z0)\nG21\nG90\nG94\nG17\nG40\nG49\nG54\nM5\nG0 Z{:.3}\nM3 S{:.0}\n", r.operation, r.clearance, r.rpm);
    let mut moves = Vec::new();
    let mut previous: Option<[f64; 3]> = None;
    for segment in path.segments {
        let (to, rapid, feed) = match segment {
            ToolpathSegment::Rapid { to } => (to, true, 0.0),
            ToolpathSegment::Linear { to, feed } => (to, false, feed),
            ToolpathSegment::Comment { .. } => continue,
            _ => return Err("Unsupported motion in toolpath".into()),
        };
        if to.iter().any(|v| !v.is_finite()) || !feed.is_finite() {
            return Err("Non-finite generated motion".into());
        }
        let margin = r.diameter / 2.0 + 0.002;
        if to[0] < -margin
            || to[0] > r.width + margin
            || to[1] < -margin
            || to[1] > r.height + margin
            || to[2] < -r.depth - 0.001
            || to[2] > r.clearance + 0.001
        {
            return Err("Generated motion exceeds the machining region".into());
        }
        if rapid {
            if let Some(from) = previous {
                if from[2] < r.clearance - 0.001 {
                    // The kernel's final rapid may combine XY with a retract.
                    // Clear the stock vertically before traversing in XY.
                    code.push_str(&format!("G0 Z{:.3}\n", r.clearance));
                    moves.push(Move {
                        to: [from[0], from[1], r.clearance],
                        rapid: true,
                        feed: 0.0,
                    });
                }
            }
        }
        code.push_str(&format!(
            "G{} X{:.3} Y{:.3} Z{:.3}",
            if rapid { 0 } else { 1 },
            to[0],
            to[1],
            to[2]
        ));
        if !rapid {
            code.push_str(&format!(" F{feed:.3}"));
        }
        code.push('\n');
        moves.push(Move { to, rapid, feed });
        previous = Some(to);
    }
    if moves.is_empty() {
        return Err("Tool does not fit the machining region".into());
    }
    code.push_str(&format!("G0 Z{:.3}\nM5\nM2\n", r.clearance));
    serde_json::to_string(&serde_json::json!({"gcode": code, "moves": moves}))
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn request(op: &str) -> serde_json::Value {
        serde_json::json!({"operation":op,"width":40,"height":30,"depth":1,
            "diameter":3.175,"stepdown":0.5,"stepover":1.5,"feed":400,
            "plunge":100,"rpm":10000,"clearance":5})
    }
    #[test]
    fn contour_jobs_follow_an_imported_polyline_with_tabs() {
        // A diamond inside a 40×30 region: outside with three tabs, inside plain.
        let diamond = serde_json::json!([[20, 3], [37, 15], [20, 27], [3, 15]]);
        for (op, tabs) in [("contour_outside", 3), ("contour_inside", 0)] {
            let mut req = request(op);
            req["contour"] = diamond.clone();
            req["tabs"] = serde_json::json!(tabs);
            req["tabWidth"] = serde_json::json!(4.0);
            req["tabHeight"] = serde_json::json!(0.5);
            let json: serde_json::Value =
                serde_json::from_str(&generate(&req.to_string()).unwrap()).unwrap();
            let moves = json["moves"].as_array().unwrap();
            assert!(
                moves.len() > 8,
                "{op}: expected a traced contour, got {}",
                moves.len()
            );
            // Every cutting move stays within the region plus the cutter radius.
            for m in moves {
                let to = m["to"].as_array().unwrap();
                let (x, y) = (to[0].as_f64().unwrap(), to[1].as_f64().unwrap());
                assert!(
                    (-1.6..=41.6).contains(&x) && (-1.6..=31.6).contains(&y),
                    "{op}: {x},{y}"
                );
            }
        }
        // A point outside the region is refused.
        let mut bad = request("contour_outside");
        bad["contour"] = serde_json::json!([[0, 0], [50, 0], [50, 30]]);
        assert!(generate(&bad.to_string()).is_err());
    }
    #[test]
    fn rectangular_jobs_have_explicit_motion_and_no_tool_change() {
        for op in ["face", "pocket", "profile"] {
            let json: serde_json::Value =
                serde_json::from_str(&generate(&request(op).to_string()).unwrap()).unwrap();
            let code = json["gcode"].as_str().unwrap();
            assert!(code.contains("G94\n") && code.ends_with("M5\nM2\n"));
            assert!(!code.contains("M6"));
            assert!(!json["moves"].as_array().unwrap().is_empty());
            let movements = json["moves"].as_array().unwrap();
            if op == "pocket" {
                assert!(
                    movements.len() > 20,
                    "pocket must clear rings, not just plunge"
                );
            }
            for pair in movements.windows(2) {
                if pair[1]["rapid"] == true && pair[0]["to"][2].as_f64().unwrap() < 5.0 {
                    assert_eq!(
                        pair[0]["to"][0], pair[1]["to"][0],
                        "retract vertically before XY travel"
                    );
                    assert_eq!(pair[0]["to"][1], pair[1]["to"][1]);
                }
            }
            for motion in movements {
                let xyz = motion["to"].as_array().unwrap();
                assert!(
                    xyz[0].as_f64().unwrap() >= -1.589 && xyz[0].as_f64().unwrap() <= 41.589,
                    "{op}: {motion}"
                );
                assert!(
                    xyz[1].as_f64().unwrap() >= -1.589 && xyz[1].as_f64().unwrap() <= 31.589,
                    "{op}: {motion}"
                );
            }
            for line in code.lines().filter(|s| s.starts_with("G1 ")) {
                assert!(
                    line.contains(" X")
                        && line.contains(" Y")
                        && line.contains(" Z")
                        && line.contains(" F")
                );
            }
        }
    }
    #[test]
    fn invalid_and_excessive_pass_counts_are_rejected() {
        for (key, value) in [("stepdown", 0.0), ("stepover", 0.000001), ("depth", -1.0)] {
            let mut r = request("face");
            r[key] = serde_json::json!(value);
            assert!(generate(&r.to_string()).is_err());
        }
    }
}
