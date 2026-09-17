//! Verify a G-code job against the DXF outline it is meant to cut.
//!
//! ```text
//! cargo run -p vcad-kernel-cam --release --example verify_nc -- \
//!     job.nc outline.dxf <tool_d> <stock_thickness> [bottom_allowance] [spoilboard]
//! ```
//!
//! The DXF is in part coordinates; the job is in the stock frame (outer loop's
//! lower-left at the origin), which is how the native app imports an outline.

use vcad_kernel_cam::outline::read_dxf;
use vcad_kernel_cam::verify2d::{verify_gcode, JobSpec, PartRegion, VerifyOptions};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        return Err(
            "usage: verify_nc <job.nc> <outline.dxf> <tool_d> <thickness> [allowance] [spoilboard]"
                .into(),
        );
    }
    let gcode = std::fs::read_to_string(&args[1])?;
    let outline = read_dxf(&std::fs::read_to_string(&args[2])?)?;
    let tool: f64 = args[3].parse()?;
    let thickness: f64 = args[4].parse()?;
    let allowance: f64 = args.get(5).map(|s| s.parse()).transpose()?.unwrap_or(0.0);
    let spoilboard = args.get(6).is_some_and(|s| s == "spoilboard");

    let region = outline
        .regions
        .iter()
        .max_by(|a, b| a.outer.area().total_cmp(&b.outer.area()))
        .ok_or("the outline has no region")?;
    let bounds = region.outer.bounds();
    let shift = |l: &vcad_kernel_cam::outline::Loop| -> Vec<[f64; 2]> {
        l.points
            .iter()
            .map(|p| [p.x - bounds[0], p.y - bounds[1]])
            .collect()
    };
    let part = PartRegion::new(
        shift(&region.outer),
        region.holes.iter().map(shift).collect(),
    )?;
    let mut spec = JobSpec::new(part, thickness, tool);
    spec.bottom_allowance = allowance;
    spec.spoilboard = spoilboard;

    let started = std::time::Instant::now();
    let report = verify_gcode(&gcode, &spec, &VerifyOptions::default())?;
    println!(
        "{} moves verified in {:.0} ms — {}",
        report.moves,
        started.elapsed().as_secs_f64() * 1e3,
        if report.pass { "PASS" } else { "FAIL" }
    );
    let line = |c: &vcad_kernel_cam::verify2d::CheckReport| {
        println!(
            "  {:<14} {:<5} {:?}  violations {:>5}  worst {:>8.4}  {}",
            c.name,
            if c.pass { "ok" } else { "FAIL" },
            c.severity,
            c.violation_count,
            c.worst,
            c.note
        );
        for v in c.examples.iter().take(3) {
            println!(
                "      #{:<6} ({:.3}, {:.3}) z {:.3}  {:.4}  {}",
                v.index, v.xy[0], v.xy[1], v.z, v.value, v.what
            );
        }
    };
    line(&report.gouge);
    line(&report.material_left.check);
    line(&report.rapids);
    line(&report.depth.check);
    line(&report.tabs.check);
    line(&report.envelope.check);
    line(&report.loose.check);
    line(&report.plunges);
    println!(
        "  tabs: {} on {} pass(es) below them; envelope X {:.2}..{:.2} Y {:.2}..{:.2}; loose pieces {} (skin holds: {})",
        report.tabs.tab_count,
        report.tabs.passes_below_tabs,
        report.envelope.work_min[0],
        report.envelope.work_max[0],
        report.envelope.work_min[1],
        report.envelope.work_max[1],
        report.loose.pieces.len(),
        report.loose.skin_holds
    );
    Ok(())
}
