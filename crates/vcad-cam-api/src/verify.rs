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
use vcad_kernel_cam::{
    fit_contour, verify_gcode, BottomAllowance, ContourSide, FitOptions, ToolReach,
};

use crate::placement::PlacementReq;
use crate::types::{loop_points, non_negative, positive, MachineReq, PartReq, StockReq};

// ---------------------------------------------------------------------------
// vcad_cam_verify_gcode
// ---------------------------------------------------------------------------

/// `{ "gcode": "…", "part": {outer, holes}, "stock": {…},
///    "tool_diameter": 2.0, "bottom_allowance": 0.15,
///    "machine": { "travel": …, "work_offset": … },
///    "tabs": [{ "width": 4, "height": 0.42 }],
///    "placement": { "dx": 15, "dy": 15, "rotation_deg": 0 },
///    "centre_cutting": true, "tolerance": 0.02 }`
#[derive(Debug, Clone, Deserialize)]
struct VerifyRequest {
    gcode: String,
    part: PartReq,
    stock: StockReq,
    tool_diameter: f64,
    /// Where the part sits on the stock, exactly as `job`'s
    /// `options.placement` means it. A program posted from a placed job is in
    /// stock coordinates while the part is stated in its own frame, so
    /// without this the replay checks the file against a part that is not
    /// where the cutter is and every cut reads as a gouge. Defaults to the
    /// identity, which is what a job with no placement posts.
    #[serde(default)]
    placement: Option<PlacementReq>,
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
    /// Per-check severity override, `{"loose_pieces": "warning"}`, exactly as
    /// `job`'s `verify_policy` means it.
    ///
    /// Without it a program that a job posted under a stated policy could not
    /// be re-verified from disk under that policy: the job says ready, the
    /// replay of its own output says blocked, and the two answers are both
    /// this crate's. An empty map is the oracle's own severities, which is
    /// what every caller got before.
    #[serde(default)]
    verify_policy: std::collections::BTreeMap<String, String>,
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
    let part = match &req.placement {
        Some(p) => req.part.placed(&p.build("placement")?)?,
        None => req.part.build()?,
    };
    let tool_diameter = positive("tool_diameter", req.tool_diameter)?;
    // Finite is not enough: the allowance has to be a distance *inside* this
    // stock. `+7` on 6 mm of plate puts the floor above the stock top, which
    // every depth reading then fails against — and on a program with no
    // cutting move the oracle had no move to name. The rule itself lives on
    // `JobSpec`; this repeats it here only to name the request field.
    let allowance = BottomAllowance(match req.bottom_allowance {
        Some(a) => {
            let a = crate::types::finite("bottom_allowance", a)?;
            if a >= stock.thickness {
                return Err(format!(
                    "bottom_allowance {a} leaves nothing to cut in {} mm of stock: a skin has to be thinner than the plate.",
                    stock.thickness
                ));
            }
            if a < -stock.thickness {
                return Err(format!(
                    "bottom_allowance {a} sinks more than the {} mm stock thickness past the underside: that is a cut into the bed, not an allowance.",
                    stock.thickness
                ));
            }
            a
        }
        None => 0.0,
    });

    // No tool library here — just a diameter someone typed — so the
    // centre-cutting fact is stated outright. Left out, it stays permissive,
    // which is what it has always been on this path.
    let mut spec = stock.job_spec(
        part,
        ToolReach::declared(tool_diameter, req.centre_cutting.unwrap_or(true)),
        allowance,
    );
    let (travel, work_offset) = req.machine.travel_limits()?;
    spec.travel = travel;
    spec.work_offset = work_offset;
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
    let (blocked_by, warnings) = crate::job::policy(&report, &req.verify_policy)?;

    Ok(json!({
        // `pass` is the oracle's own verdict, untouched by the policy: a
        // caller that wants to know whether anything failed at all still can.
        "pass": report.pass,
        "blocked": !blocked_by.is_empty(),
        "policy": { "verified": true, "blocked_by": blocked_by, "warnings": warnings },
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
