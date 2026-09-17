//! `vcad_cam_verify_gcode` and `vcad_cam_fit`.
//!
//! The first answers the question the app could not ask on the day of the
//! first cut: *is the file I am about to send the one that makes this part?*
//! It takes G-code as text — an imported program, a file off disk, whatever
//! is in the sender — and replays it against the part, the stock and the
//! tool. The second answers item 38: *does this cutter fit?*

use serde::Deserialize;
use serde_json::{json, Value};

use vcad_kernel_cam::verify2d::{DeclaredTab, VerifyOptions};
use vcad_kernel_cam::{fit_contour, verify_gcode, BottomAllowance, ContourSide, FitOptions};

use crate::types::{loop_points, non_negative, positive, MachineReq, PartReq, StockReq};

// ---------------------------------------------------------------------------
// vcad_cam_verify_gcode
// ---------------------------------------------------------------------------

/// `{ "gcode": "…", "part": {outer, holes}, "stock": {…},
///    "tool_diameter": 2.0, "bottom_allowance": 0.15,
///    "machine": { "travel": …, "work_offset": … },
///    "tabs": [{ "width": 4, "height": 0.42 }],
///    "centre_cutting": true, "tolerance": 0.02 }`
#[derive(Debug, Clone, Deserialize)]
struct VerifyRequest {
    gcode: String,
    part: PartReq,
    stock: StockReq,
    tool_diameter: f64,
    #[serde(default)]
    bottom_allowance: Option<f64>,
    #[serde(default)]
    machine: MachineReq,
    #[serde(default)]
    tabs: Vec<DeclaredTabReq>,
    #[serde(default)]
    centre_cutting: Option<bool>,
    #[serde(default)]
    tolerance: Option<f64>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
struct DeclaredTabReq {
    width: f64,
    height: f64,
}

/// Verify G-code text against the part it is meant to make.
///
/// Answers the same `verification` document `vcad_cam_job` carries, plus
/// `pass` and `blocked` at the top level so a caller that only wants the
/// verdict does not have to walk eight checks.
pub fn verify_gcode_request(input: &str) -> Result<Value, String> {
    let req: VerifyRequest = serde_json::from_str(input).map_err(|e| {
        format!("the verification request could not be read: {e}. Expected gcode, part, stock and tool_diameter.")
    })?;
    if req.gcode.trim().is_empty() {
        return Err("the G-code is empty: there is nothing to verify.".into());
    }
    let stock = req.stock.build()?;
    let part = req.part.build()?;
    let tool_diameter = positive("tool_diameter", req.tool_diameter)?;
    let allowance = BottomAllowance(match req.bottom_allowance {
        Some(a) => crate::types::finite("bottom_allowance", a)?,
        None => 0.0,
    });

    let mut spec = stock.job_spec(part, tool_diameter, allowance);
    let (travel, work_offset) = req.machine.travel_limits()?;
    spec.travel = travel;
    spec.work_offset = work_offset;
    if let Some(c) = req.centre_cutting {
        spec.centre_cutting = c;
    }
    spec.declared_tabs = req
        .tabs
        .iter()
        .map(|t| {
            Ok(DeclaredTab {
                width: positive("tabs[].width", t.width)?,
                height: positive("tabs[].height", t.height)?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;

    let mut opts = VerifyOptions::default();
    if let Some(t) = req.tolerance {
        opts.tolerance = positive("tolerance", t)?;
    }

    let report = verify_gcode(&req.gcode, &spec, &opts)
        .map_err(|e| format!("this G-code could not be replayed: {e}"))?;
    let blocked_by: Vec<String> = [
        &report.gouge,
        &report.material_left.check,
        &report.rapids,
        &report.depth.check,
        &report.tabs.check,
        &report.envelope.check,
        &report.loose.check,
        &report.plunges,
    ]
    .iter()
    .filter(|c| !c.pass && c.severity == vcad_kernel_cam::verify2d::Severity::Error)
    .map(|c| c.name.clone())
    .collect();

    Ok(json!({
        "pass": report.pass,
        "blocked": !blocked_by.is_empty(),
        "policy": { "verified": true, "blocked_by": blocked_by },
        "verification": report,
    }))
}

// ---------------------------------------------------------------------------
// vcad_cam_fit
// ---------------------------------------------------------------------------

/// `{ "contour": [[x,y], …], "tool_diameter": 3.175, "side": "inside",
///    "grid": 0.02, "min_area": 0.01 }`
#[derive(Debug, Clone, Deserialize)]
struct FitRequest {
    contour: Vec<[f64; 2]>,
    tool_diameter: f64,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    grid: Option<f64>,
    #[serde(default)]
    min_area: Option<f64>,
}

/// Cutter-fit report for one contour, one tool and one side, plus the largest
/// tool that still passes — the number that answers "so what should I use?".
pub fn fit_request(input: &str) -> Result<Value, String> {
    let req: FitRequest = serde_json::from_str(input).map_err(|e| {
        format!("the fit request could not be read: {e}. Expected contour, tool_diameter and side.")
    })?;
    let points = loop_points("contour", &req.contour)?;
    let diameter = positive("tool_diameter", req.tool_diameter)?;
    let side = match req.side.as_deref().map(str::to_ascii_lowercase).as_deref() {
        None | Some("inside") => ContourSide::Inside,
        Some("outside") => ContourSide::Outside,
        Some(other) => {
            return Err(format!(
                "side is \"{other}\": it has to be \"inside\" (the cutter works within the loop) or \"outside\"."
            ))
        }
    };
    let mut opts = FitOptions::default();
    if let Some(g) = req.grid {
        opts.grid = positive("grid", g)?;
    }
    if let Some(a) = req.min_area {
        opts.min_area = non_negative("min_area", a)?;
    }
    let report = fit_contour(&points, diameter, side, &opts)
        .map_err(|e| format!("the cutter fit could not be worked out: {e}"))?;
    Ok(json!({
        "fits": report.fits,
        "side": match side { ContourSide::Inside => "inside", ContourSide::Outside => "outside" },
        "largest_tool_diameter": report.largest_tool_diameter,
        "report": report,
    }))
}
