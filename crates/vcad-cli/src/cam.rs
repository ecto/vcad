//! `vcad cam` — the CAM surface, on the command line.
//!
//! Everything that decides anything lives in [`vcad_cam_api`]: one
//! implementation of "is this job blocked?" behind the app's C ABI, the
//! browser's WASM, an agent's MCP tools and this. What belongs here is the
//! part a CLI owes its caller — read a request, render the answer so a
//! machinist can read it, and **exit with a code that means what it says**.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | the answer is yes: the job posted, the program verified, the outlines agree |
//! | 1 | the request could not be **read** — no such file, not JSON, not a document this command can section |
//! | 2 | it was read, and the answer is **no**: the job is blocked or refused, the G-code fails verification, the cutter does not fit, the section is torn, the outlines disagree |
//!
//! The line between 1 and 2 is whether anything was understood. A 1 means the
//! file never got as far as the CAM surface; a 2 means it did, and came back
//! with a reason — a failed check, a cut past the stock, a number the surface
//! refuses by name. Both leave nothing on disk to run: a blocked `job` writes
//! no `.nc`, because a file that exists is a file that can be sent.

use std::io::Read;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use serde_json::{json, Value};

/// It ran, and the answer is no.
pub const EXIT_REFUSED: i32 = 2;

/// Flags every `vcad cam` subcommand shares.
#[derive(Args, Clone, Debug)]
pub struct CamCommon {
    /// Request JSON. Omit it, or pass `-`, to read the request from stdin.
    #[arg(long, value_name = "FILE", global = true)]
    pub request: Option<PathBuf>,

    /// Write the whole JSON answer here. Written whatever the verdict —
    /// a refusal is exactly the answer worth keeping.
    #[arg(long, value_name = "FILE", global = true)]
    pub out: Option<PathBuf>,

    /// Print the whole JSON answer on stdout instead of the human summary.
    #[arg(long, global = true)]
    pub json: bool,

    /// Print the human summary on stdout (the default).
    #[arg(long, global = true, conflicts_with = "json")]
    pub summary: bool,
}

/// The CAM entry points, one subcommand each.
#[derive(Subcommand, Clone, Debug)]
pub enum CamCommand {
    /// Post a whole job: operations, tools, verification, G-code
    ///
    /// The G-code is written ONLY when the job is not blocked. A blocked job
    /// exits 2, names the failed checks on stderr, and writes no `--gcode`
    /// file — there is nothing to send by accident.
    Job {
        /// Write the posted program here (only ever on a job that passed)
        #[arg(long, value_name = "FILE")]
        gcode: Option<PathBuf>,
    },

    /// Replay a G-code program against the part it is meant to make
    ///
    /// The program can be inline in the request as `"gcode"`, or handed over
    /// with `--gcode`, which overrides it.
    Verify {
        /// Read the program from this file instead of the request's `gcode`
        #[arg(long, value_name = "FILE")]
        gcode: Option<PathBuf>,
    },

    /// Does this cutter fit this contour?
    Fit,

    /// Section a solid (or an STL) to a closed contour at a Z plane
    ///
    /// A `.vcad` or `.loon` input is evaluated exactly as `export` and `info`
    /// evaluate it, and then the RAW tessellation of the B-rep is sectioned —
    /// never the export mesh, whose repair pass moves vertices onto their
    /// analytic carriers and sections up to 0.4 mm away from the wall the
    /// cutter has to follow.
    ///
    /// A section that does not close is refused with its gaps: the solid is
    /// torn there, and healing the outline would hide it.
    Outline {
        /// Input `.vcad`, `.loon` or `.stl`
        input: PathBuf,
        /// Which part of the evaluated scene (default: 0)
        #[arg(long, default_value = "0")]
        part: usize,
        /// Section plane, mm. Without it the mid-height of the part is used.
        #[arg(long, value_name = "MM")]
        z: Option<f64>,
    },

    /// Are these two outlines the same part?
    ///
    /// The check that catches a stale DXF before it machines something else.
    Compare {
        /// DXF to compare (side `a`)
        #[arg(long, value_name = "FILE")]
        dxf: Option<PathBuf>,
        /// The other side (`b`): a `cam outline` answer, or a `.vcad`,
        /// `.loon` or `.stl` that is sectioned on the way in
        #[arg(long, value_name = "FILE")]
        outline: Option<PathBuf>,
        /// Section plane for an `--outline` that is a model, mm
        #[arg(long, value_name = "MM")]
        z: Option<f64>,
        /// How far apart two boundaries may be and still be the same part, mm
        #[arg(long, value_name = "MM")]
        tolerance: Option<f64>,
    },

    /// The material table (takes no request)
    Materials,

    /// Feeds, speeds, stepdown, stepover and the router dial to set
    Recommend,

    /// A second opinion on feeds and speeds the operator already has
    CheckFeeds,

    /// Gear geometry: reachability, contours, tool-centre paths, over-pins
    Gear,
}

/// Run one `vcad cam` subcommand. Returns the process exit code.
pub fn run(command: &CamCommand, common: &CamCommon) -> Result<i32> {
    match command {
        CamCommand::Job { gcode } => job(common, gcode.as_deref()),
        CamCommand::Verify { gcode } => verify(common, gcode.as_deref()),
        CamCommand::Fit => fit(common),
        CamCommand::Outline { input, part, z } => outline(common, input, *part, *z),
        CamCommand::Compare {
            dxf,
            outline: other,
            z,
            tolerance,
        } => compare(common, dxf.as_deref(), other.as_deref(), *z, *tolerance),
        CamCommand::Materials => materials(common),
        CamCommand::Recommend => recommend(common),
        CamCommand::CheckFeeds => check_feeds(common),
        CamCommand::Gear => gear(common),
    }
}

// ---------------------------------------------------------------------------
// Request and answer plumbing
// ---------------------------------------------------------------------------

/// The request text: a file, or stdin.
fn request_text(common: &CamCommon) -> Result<String> {
    let text = match &common.request {
        Some(p) if p != Path::new("-") => std::fs::read_to_string(p)
            .with_context(|| format!("reading the request {}", p.display()))?,
        _ => {
            let mut buffer = String::new();
            std::io::stdin()
                .read_to_string(&mut buffer)
                .context("reading the request from stdin")?;
            buffer
        }
    };
    if text.trim().is_empty() {
        bail!("the request is empty: pass --request <FILE>, or send the JSON on stdin.");
    }
    Ok(text)
}

/// The request, parsed, for the commands that fill fields in from flags.
fn request_value(common: &CamCommon) -> Result<Value> {
    let text = request_text(common)?;
    serde_json::from_str(&text).context("the request is not valid JSON")
}

/// Everything after the call: write `--out`, print the answer, hand back the
/// exit code.
fn finish(
    common: &CamCommon,
    answer: &Value,
    summary: impl FnOnce(&Value),
    code: i32,
) -> Result<i32> {
    if let Some(path) = &common.out {
        let text = serde_json::to_string_pretty(answer)?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    if common.json {
        println!("{}", serde_json::to_string_pretty(answer)?);
    } else {
        summary(answer);
    }
    Ok(code)
}

/// The CAM surface refused. The request had already parsed, so this is the
/// answer being no rather than a file that could not be read: nothing is
/// written, and the message says what to change.
fn refused(common: &CamCommon, message: &str) -> Result<i32> {
    eprintln!("REFUSED: {message}");
    let answer = json!({ "error": message, "blocked": true });
    finish(
        common,
        &answer,
        |a| println!("REFUSED: {}", s(a, "error")),
        EXIT_REFUSED,
    )
}

/// Run one entry point over a request that has already been read as JSON.
/// `Ok(Ok(v))` is an answer; `Ok(Err(code))` is a refusal already reported.
macro_rules! call {
    ($common:expr, $entry:path, $request:expr) => {
        match $entry(&$request.to_string()) {
            Ok(v) => v,
            Err(e) => return refused($common, &e),
        }
    };
}

fn f(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(Value::as_f64).unwrap_or(f64::NAN)
}

fn s(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(t)) => t.clone(),
        Some(other) => other.to_string(),
        None => "—".into(),
    }
}

/// `name` at `path.to.here`, or `Value::Null`.
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    let mut cursor = v;
    for step in path.split('.') {
        cursor = match cursor.get(step) {
            Some(next) => next,
            None => return &Value::Null,
        };
    }
    cursor
}

/// Print the `notes` list every CAM answer may carry.
fn print_notes(answer: &Value) {
    let Some(notes) = answer.get("notes").and_then(Value::as_array) else {
        return;
    };
    if notes.is_empty() {
        return;
    }
    println!("\nnotes");
    for n in notes {
        println!("  [{}] {}", s(n, "level"), s(n, "text"));
    }
}

/// One line per verification check, with the number it turned on.
fn print_checks(verification: &Value) {
    let names = [
        ("gouge", "gouge"),
        ("material_left", "material_left.check"),
        ("rapids", "rapids"),
        ("depth", "depth.check"),
        ("tabs", "tabs.check"),
        ("envelope", "envelope.check"),
        ("loose", "loose.check"),
        ("plunges", "plunges"),
    ];
    println!(
        "  {:<16} {:<6} {:<9} {:>5} {:>10}",
        "check", "verdict", "severity", "n", "worst"
    );
    for (label, path) in names {
        let c = at(verification, path);
        if c.is_null() {
            continue;
        }
        println!(
            "  {:<16} {:<6} {:<9} {:>5} {:>10.4}",
            label,
            if c.get("pass") == Some(&Value::Bool(true)) {
                "pass"
            } else {
                "FAIL"
            },
            s(c, "severity"),
            c.get("violation_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            f(c, "worst"),
        );
    }
}

/// The failed checks, in the words the oracle used, on stderr.
fn print_refusal(answer: &Value) {
    let blocked_by = at(answer, "policy.blocked_by");
    let names: Vec<String> = blocked_by
        .as_array()
        .map(|a| {
            a.iter()
                .map(|v| v.as_str().unwrap_or("?").to_string())
                .collect()
        })
        .unwrap_or_default();
    if let Some(error) = answer.get("error").and_then(Value::as_str) {
        eprintln!("REFUSED: {error}");
    } else {
        eprintln!("REFUSED: {}", names.join(", "));
    }
    let verification = answer.get("verification");
    for name in &names {
        let Some(v) = verification else { continue };
        let path = match name.as_str() {
            "material_left" => "material_left.check",
            "depth" => "depth.check",
            "tabs" => "tabs.check",
            "envelope" => "envelope.check",
            "loose_pieces" | "loose" => "loose.check",
            other => other,
        };
        let c = at(v, path);
        if c.is_null() {
            eprintln!("  {name}");
            continue;
        }
        eprintln!(
            "  {name}: {} violation(s), worst {:.4} — {}",
            c.get("violation_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            f(c, "worst"),
            s(c, "note")
        );
        for example in c
            .get("examples")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(3)
        {
            eprintln!(
                "    · {}",
                serde_json::to_string(example).unwrap_or_default()
            );
        }
    }
    for n in answer
        .get("notes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if matches!(s(n, "level").as_str(), "danger" | "Danger") {
            eprintln!("  ! {}", s(n, "text"));
        }
    }
    eprintln!("  no G-code was written.");
}

// ---------------------------------------------------------------------------
// job
// ---------------------------------------------------------------------------

fn job(common: &CamCommon, gcode_out: Option<&Path>) -> Result<i32> {
    let request = request_value(common)?;
    let answer = call!(common, vcad_cam_api::job_value, request);
    let blocked = answer.get("blocked") == Some(&Value::Bool(true));

    // The whole point of the fail-closed rule: the file is written inside the
    // `if`, so a blocked job leaves nothing on disk to send. Writing whatever
    // came back and letting it be empty would put a zero-byte program on disk,
    // which a sender opens quite happily.
    if !blocked {
        if let (Some(path), Some(gcode)) = (gcode_out, answer.get("gcode").and_then(Value::as_str))
        {
            std::fs::write(path, gcode).with_context(|| format!("writing {}", path.display()))?;
        } else if gcode_out.is_some() {
            bail!("the job passed but carries no G-code: nothing to write.");
        }
    }

    let code = if blocked { EXIT_REFUSED } else { 0 };
    if blocked {
        print_refusal(&answer);
    }
    let gcode_path = gcode_out.map(|p| p.to_path_buf());
    finish(
        common,
        &answer,
        move |a| job_summary(a, gcode_path.as_deref()),
        code,
    )
}

fn job_summary(answer: &Value, gcode_out: Option<&Path>) {
    println!("job: {}", s(answer, "name"));
    let blocked = answer.get("blocked") == Some(&Value::Bool(true));
    println!(
        "  verdict: {}",
        if blocked {
            "BLOCKED — no G-code"
        } else {
            "ready"
        }
    );
    if let Some(tools) = answer.get("tool_sequence").and_then(Value::as_array) {
        let list: Vec<String> = tools.iter().map(|t| format!("T{t}")).collect();
        println!("  tools: {}", list.join(" → "));
    }
    let accel = f(at(answer, "duration"), "accel_aware_s");
    if accel.is_finite() {
        println!(
            "  cutting time: {:.1} s ({:.1} min, acceleration-aware)",
            accel,
            accel / 60.0
        );
    }
    if let Some(moves) = answer.get("moves").and_then(Value::as_array) {
        println!("  preview moves: {}", moves.len());
    }
    if let Some(arc) = answer.get("arc_fit") {
        println!(
            "  arc fit: {} segment(s) in → {} out, {} arc(s), worst deviation {:.5} mm",
            arc.get("segments_in").and_then(Value::as_u64).unwrap_or(0),
            arc.get("segments_out").and_then(Value::as_u64).unwrap_or(0),
            arc.get("arcs_emitted").and_then(Value::as_u64).unwrap_or(0),
            f(arc, "max_deviation"),
        );
    }

    let ops: Vec<&Value> = answer
        .get("op_ranges")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter(|r| s(r, "block") == "operation").collect())
        .unwrap_or_default();
    if !ops.is_empty() {
        println!("\noperations ({})", ops.len());
        println!(
            "  {:<34} {:<5} {:>8} {:>8}",
            "name", "tool", "moves", "seconds"
        );
        for r in ops.iter().take(24) {
            let start = r.get("start").and_then(Value::as_u64).unwrap_or(0);
            let end = r.get("end").and_then(Value::as_u64).unwrap_or(0);
            println!(
                "  {:<34} {:<5} {:>8} {:>8.2}",
                s(r, "name"),
                s(r, "tool"),
                end.saturating_sub(start),
                f(r, "seconds")
            );
        }
        if ops.len() > 24 {
            println!("  … and {} more", ops.len() - 24);
        }
    }

    if let Some(v) = answer.get("verification") {
        println!(
            "\nverification ({} moves replayed, {})",
            v.get("moves").and_then(Value::as_u64).unwrap_or(0),
            s(at(answer, "policy"), "replayed")
        );
        print_checks(v);
    }
    let warnings = at(answer, "policy.warnings");
    if let Some(list) = warnings.as_array() {
        if !list.is_empty() {
            let names: Vec<String> = list
                .iter()
                .map(|v| v.as_str().unwrap_or("?").into())
                .collect();
            println!("  warnings (do not block): {}", names.join(", "));
        }
    }
    for tab in answer
        .get("tab_placement")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "\ntabs on {}: {} asked for, {} found on a {:.2} mm perimeter",
            s(tab, "op"),
            tab.get("requested").and_then(Value::as_u64).unwrap_or(0),
            tab.get("found").and_then(Value::as_u64).unwrap_or(0),
            f(tab, "perimeter_mm"),
        );
        for t in tab
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            println!(
                "  metal {:.3} mm wide, {:.3} mm tall, {:.2} mm to the next, {}",
                f(t, "metal_width"),
                f(t, "height"),
                f(t, "gap_to_next_mm"),
                if t.get("straight") == Some(&Value::Bool(true)) {
                    "on a straight run"
                } else {
                    "on a curve"
                },
            );
        }
    }
    print_notes(answer);
    if let Some(gcode) = answer.get("gcode").and_then(Value::as_str) {
        println!("\nG-code: {} line(s)", gcode.lines().count());
        if let Some(path) = gcode_out {
            println!("  written to {}", path.display());
        }
    }
}

// ---------------------------------------------------------------------------
// verify
// ---------------------------------------------------------------------------

fn verify(common: &CamCommon, gcode_in: Option<&Path>) -> Result<i32> {
    let mut request = request_value(common)?;
    if let Some(path) = gcode_in {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the program {}", path.display()))?;
        request["gcode"] = json!(text);
    }
    let answer = call!(common, vcad_cam_api::verify_gcode_value, request);
    // `pass` is the oracle's own verdict; `blocked` is that verdict after the
    // request's `verify_policy`. The exit code follows `blocked`, because that
    // is the question the caller asked — and a demoted check still shows up in
    // the summary as a failure, so nothing is hidden.
    let blocked = answer.get("blocked") == Some(&Value::Bool(true));
    if blocked {
        print_refusal(&answer);
    }
    finish(
        common,
        &answer,
        verify_summary,
        if blocked { EXIT_REFUSED } else { 0 },
    )
}

fn verify_summary(answer: &Value) {
    let pass = answer.get("pass") == Some(&Value::Bool(true));
    let blocked = answer.get("blocked") == Some(&Value::Bool(true));
    println!(
        "verification: {}",
        match (pass, blocked) {
            (true, _) => "PASS",
            (false, false) =>
                "PASS under the stated verify_policy (a check failed and was demoted)",
            (false, true) => "FAIL",
        }
    );
    if let Some(v) = answer.get("verification") {
        println!(
            "  {} move(s) replayed",
            v.get("moves").and_then(Value::as_u64).unwrap_or(0)
        );
        print_checks(v);
    }
    if let Some(list) = at(answer, "policy.warnings").as_array() {
        if !list.is_empty() {
            let names: Vec<String> = list
                .iter()
                .map(|v| v.as_str().unwrap_or("?").into())
                .collect();
            println!(
                "  demoted to warnings by verify_policy: {}",
                names.join(", ")
            );
        }
    }
}

// ---------------------------------------------------------------------------
// fit
// ---------------------------------------------------------------------------

fn fit(common: &CamCommon) -> Result<i32> {
    let request = request_value(common)?;
    let answer = call!(common, vcad_cam_api::fit_value, request);
    let fits = answer.get("fits") == Some(&Value::Bool(true));
    if !fits {
        let report = at(&answer, "report.unreachable");
        eprintln!(
            "REFUSED: this cutter does not fit — {} unreachable corner(s), {:.4} mm² in all, worst stand-off {:.4} mm. The largest tool that does fit is Ø{:.4}.",
            report.get("count").and_then(Value::as_u64).unwrap_or(0),
            f(report, "total_area"),
            f(report, "max_standoff"),
            f(&answer, "largest_tool_diameter"),
        );
    }
    let code = if fits { 0 } else { EXIT_REFUSED };
    finish(common, &answer, fit_summary, code)
}

fn fit_summary(answer: &Value) {
    println!(
        "cutter fit ({} the contour): {}",
        s(answer, "side"),
        if answer.get("fits") == Some(&Value::Bool(true)) {
            "fits"
        } else {
            "DOES NOT FIT"
        }
    );
    println!(
        "  largest tool that fits: Ø{:.4} mm",
        f(answer, "largest_tool_diameter")
    );
    let u = at(answer, "report.unreachable");
    if !u.is_null() {
        println!(
            "  unreachable: {} corner(s), {:.4} mm², worst stand-off {:.4} mm",
            u.get("count").and_then(Value::as_u64).unwrap_or(0),
            f(u, "total_area"),
            f(u, "max_standoff"),
        );
    }
    let necks = at(answer, "report.necks");
    if let Some(list) = necks.as_array() {
        if !list.is_empty() {
            println!("  {} neck(s) the cutter cannot pass", list.len());
        }
    }
}

// ---------------------------------------------------------------------------
// outline
// ---------------------------------------------------------------------------

fn outline(common: &CamCommon, input: &Path, part: usize, z: Option<f64>) -> Result<i32> {
    // Section options ride in the request when one was given; most runs need
    // none, so an absent `--request` is not an error here.
    let options = match &common.request {
        Some(p) if p != Path::new("-") => std::fs::read_to_string(p)
            .with_context(|| format!("reading the section options {}", p.display()))?,
        Some(_) => request_text(common)?,
        None => "{}".to_string(),
    };
    let answer = section_input(input, part, z, &options).map_err(|e| anyhow::anyhow!("{e}"))?;
    let torn = answer.get("error").is_some();
    if torn {
        eprintln!("REFUSED: {}", s(&answer, "error"));
        for gap in answer
            .get("gaps")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .take(8)
        {
            eprintln!(
                "  gap {:.4} mm at {}",
                f(gap, "distance"),
                serde_json::to_string(gap).unwrap_or_default()
            );
        }
    }
    let code = if torn { EXIT_REFUSED } else { 0 };
    finish(common, &answer, outline_summary, code)
}

/// Section whatever the caller pointed at: a `.vcad`/`.loon` document through
/// the kernel, or an STL as it stands.
pub fn section_input(
    input: &Path,
    part: usize,
    z: Option<f64>,
    options: &str,
) -> Result<Value, String> {
    let auto_z = z.is_none();
    let z = z.unwrap_or(0.0);
    let extension = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "stl" {
        let (positions, indices) = read_stl(input)?;
        return vcad_cam_api::section_mesh(&positions, &indices, z, auto_z, options, "stl");
    }

    let doc = crate::load_doc(input).map_err(|e| format!("{e}"))?;
    let scene = crate::app::evaluate_scene(&doc).map_err(|e| format!("{e}"))?;
    let evaluated = scene.parts.get(part).ok_or_else(|| {
        format!(
            "this document evaluates to {} part(s), so there is no part {part}.",
            scene.parts.len()
        )
    })?;

    // The raw tessellation wherever there is topology to build it from. The
    // export mesh has been through `repair_export_mesh`, which moves vertices
    // onto their analytic carriers to close the boundary for printing; a plane
    // through the moved region cuts a slightly different shape — measured at
    // 0.4 mm on the stator, against 0.005 mm for the raw tessellation. A CAM
    // contour is a wall the cutter follows, so that is not a rounding
    // difference. Same rule, same words, as `vcad-ffi/src/cam/scene.rs`.
    if let Some(brep) = evaluated.solid.as_ref().and_then(|s| s.as_brep()) {
        let segments = vcad_cam_api::section_segments(options)?;
        let mesh = vcad_kernel_tessellate::tessellate_brep(brep, segments);
        let positions: Vec<[f64; 3]> = mesh
            .vertices
            .as_chunks::<3>()
            .0
            .iter()
            .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
            .collect();
        return vcad_cam_api::section_mesh(
            &positions,
            &mesh.indices,
            z,
            auto_z,
            options,
            "raw_tessellation",
        );
    }

    // No B-rep: an imported mesh, or a root-mesh cache hit, which lands here
    // as triangles with no topology. Sectioning it is not wrong, only less
    // exact — and `mesh_source` says so rather than letting the caller assume
    // the better path was taken.
    let cached = scene.root_keys.get(part).and_then(|k| k.as_ref()).is_some();
    let source = if cached {
        "cached_root_mesh"
    } else {
        "export_mesh"
    };
    let positions: Vec<[f64; 3]> = evaluated
        .mesh
        .positions
        .as_chunks::<3>()
        .0
        .iter()
        .map(|c| [c[0] as f64, c[1] as f64, c[2] as f64])
        .collect();
    vcad_cam_api::section_mesh(
        &positions,
        &evaluated.mesh.indices,
        z,
        auto_z,
        options,
        source,
    )
}

fn read_stl(path: &Path) -> Result<(Vec<[f64; 3]>, Vec<u32>), String> {
    let file =
        std::fs::File::open(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let mut reader = std::io::BufReader::new(file);
    let stl = stl_io::read_stl(&mut reader)
        .map_err(|e| format!("{} is not a readable STL: {e}", path.display()))?;
    if stl.faces.is_empty() {
        return Err(format!("{} contains no triangles.", path.display()));
    }
    let positions: Vec<[f64; 3]> = stl
        .vertices
        .iter()
        .map(|v| [v[0] as f64, v[1] as f64, v[2] as f64])
        .collect();
    let indices: Vec<u32> = stl
        .faces
        .iter()
        .flat_map(|t| t.vertices.iter().map(|i| *i as u32).collect::<Vec<_>>())
        .collect();
    Ok((positions, indices))
}

fn outline_summary(answer: &Value) {
    if let Some(error) = answer.get("error").and_then(Value::as_str) {
        println!("outline: REFUSED — {error}");
        return;
    }
    println!("outline from the {}", s(answer, "mesh_source"));
    let range = answer.get("z_range").and_then(Value::as_array);
    println!(
        "  z {:.4} mm (part spans {} mm), suggested stock {:.4} mm",
        f(answer, "z"),
        range
            .map(|r| format!(
                "{:.4} … {:.4}",
                r[0].as_f64().unwrap_or(f64::NAN),
                r[1].as_f64().unwrap_or(f64::NAN)
            ))
            .unwrap_or_else(|| "?".into()),
        f(answer, "suggested_stock_thickness"),
    );
    println!("  area {:.4} mm²", f(answer, "area"));
    if let Some(regions) = answer.get("regions").and_then(Value::as_array) {
        for (i, r) in regions.iter().enumerate() {
            let outer = r
                .get("outer")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            let holes = r
                .get("holes")
                .and_then(Value::as_array)
                .map(|a| a.len())
                .unwrap_or(0);
            println!(
                "  region {i}: outer {outer} point(s), {holes} hole(s), area {:.4} mm²",
                f(r, "area")
            );
        }
    }
    for c in answer
        .get("circles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let centre = c.get("center").and_then(Value::as_array);
        println!(
            "  circular hole Ø{:.4} mm at ({}), fit rms {:.5} / max {:.5} mm",
            f(c, "diameter"),
            centre
                .map(|p| format!(
                    "{:.3}, {:.3}",
                    p[0].as_f64().unwrap_or(f64::NAN),
                    p[1].as_f64().unwrap_or(f64::NAN)
                ))
                .unwrap_or_else(|| "?".into()),
            f(c, "rms_error"),
            f(c, "max_error"),
        );
    }
    let prismatic = answer.get("prismatic");
    match prismatic {
        Some(Value::Null) | None => {}
        Some(p) if p.get("refused").is_some() => println!("  prismatic: no — {}", s(p, "refused")),
        Some(p) => println!(
            "  prismatic: {} (worst departure {:.5} mm)",
            if p.get("prismatic") == Some(&Value::Bool(true)) {
                "yes"
            } else {
                "no"
            },
            f(p, "max_deviation"),
        ),
    }
    if answer.get("healed") == Some(&Value::Bool(true)) {
        println!("  the section had to be healed to close.");
    }
}

// ---------------------------------------------------------------------------
// compare
// ---------------------------------------------------------------------------

fn compare(
    common: &CamCommon,
    dxf: Option<&Path>,
    other: Option<&Path>,
    z: Option<f64>,
    tolerance: Option<f64>,
) -> Result<i32> {
    let mut request: Value = if dxf.is_none() && other.is_none() {
        request_value(common)?
    } else {
        json!({})
    };
    if let Some(path) = dxf {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading the DXF {}", path.display()))?;
        request["a"] = json!({ "dxf": text });
    }
    if let Some(path) = other {
        request["b"] = json!({ "outline": outline_of(path, z)? });
    }
    if let Some(t) = tolerance {
        request["tolerance"] = json!(t);
    }
    let answer = call!(common, vcad_cam_api::compare_outline_value, request);
    let agrees = answer.get("agrees") == Some(&Value::Bool(true));
    if !agrees {
        eprintln!("REFUSED: {}", s(&answer, "note"));
    }
    let code = if agrees { 0 } else { EXIT_REFUSED };
    finish(common, &answer, compare_summary, code)
}

/// The `outline` document for a path that is either a `cam outline` answer
/// already, or a model to section on the way in.
fn outline_of(path: &Path, z: Option<f64>) -> Result<Value> {
    let extension = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "json" {
        let text =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let value: Value = serde_json::from_str(&text)
            .with_context(|| format!("{} is not valid JSON", path.display()))?;
        // Either a whole `cam outline` answer, or the bare outline document.
        return Ok(value.get("outline").cloned().unwrap_or(value));
    }
    let sectioned = section_input(path, 0, z, "{}").map_err(|e| anyhow::anyhow!("{e}"))?;
    if let Some(error) = sectioned.get("error").and_then(Value::as_str) {
        bail!("{} could not be sectioned: {error}", path.display());
    }
    sectioned
        .get("outline")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{} sectioned to no outline", path.display()))
}

fn compare_summary(answer: &Value) {
    let diff = at(answer, "diff");
    println!(
        "outlines: {}",
        if answer.get("agrees") == Some(&Value::Bool(true)) {
            "the same part"
        } else {
            "NOT the same part"
        }
    );
    println!("  tolerance {:.4} mm", f(answer, "tolerance"));
    println!(
        "  boundaries at most {:.4} mm apart",
        f(diff, "max_boundary_distance")
    );
    let unmatched_a = diff
        .get("unmatched_a")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    let unmatched_b = diff
        .get("unmatched_b")
        .and_then(Value::as_array)
        .map(|a| a.len())
        .unwrap_or(0);
    println!("  holes on one side only: {unmatched_a} in a, {unmatched_b} in b");
    println!("  {}", s(answer, "note"));
}

// ---------------------------------------------------------------------------
// materials, recommend, check-feeds
// ---------------------------------------------------------------------------

fn materials(common: &CamCommon) -> Result<i32> {
    let answer: Value = vcad_cam_api::materials_value();
    finish(common, &answer, materials_summary, 0)
}

fn materials_summary(answer: &Value) {
    println!(
        "{:<20} {:<34} {:>10} {:>10}",
        "id", "name", "chipload", "coolant"
    );
    for m in answer
        .get("materials")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        println!(
            "{:<20} {:<34} {:>10.4} {:>10}",
            s(m, "id"),
            s(m, "name"),
            f(m, "chipload_ref_mm"),
            s(m, "coolant"),
        );
    }
    println!(
        "\noperations: {}\nmachine classes: {}\nspindles: {}",
        list(at(answer, "operations")),
        list(at(answer, "machine_classes")),
        list(at(answer, "spindles")),
    );
}

fn list(v: &Value) -> String {
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|x| x.as_str().unwrap_or("?").to_string())
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn recommend(common: &CamCommon) -> Result<i32> {
    let request = request_value(common)?;
    let answer = call!(common, vcad_cam_api::recommend_value, request);
    finish(common, &answer, recommend_summary, 0)
}

fn recommend_summary(answer: &Value) {
    let r = at(answer, "recommendation");
    println!(
        "{} on {} — {}",
        s(at(answer, "material"), "name"),
        s(at(answer, "machine"), "name"),
        s(r, "op"),
    );
    println!(
        "  Ø{:.3} mm, {} flute(s)",
        f(r, "tool_diameter_mm"),
        r.get("flutes").and_then(Value::as_u64).unwrap_or(0)
    );
    println!("  rpm            {:.0}", f(r, "rpm"));
    if let Some(dial) = r.get("dial").and_then(Value::as_str) {
        println!(
            "  DIAL           {dial}   (the S word does nothing on this spindle — set it by hand)"
        );
    }
    println!("  surface speed  {:.1} m/min", f(r, "surface_speed_m_min"));
    println!("  chipload       {:.4} mm/tooth", f(r, "chipload_mm"));
    println!("  feed           {:.0} mm/min", f(r, "feed_mm_min"));
    println!("  plunge         {:.0} mm/min", f(r, "plunge_mm_min"));
    println!("  ramp angle     {:.1}°", f(r, "ramp_angle_deg"));
    println!("  stepdown       {:.3} mm", f(r, "stepdown_mm"));
    println!("  stepover       {:.3} mm", f(r, "stepover_mm"));
    println!("  finish left    {:.3} mm", f(r, "finish_allowance_mm"));
    println!("  coolant        {}", s(r, "coolant"));
    print_notes(r);
}

fn check_feeds(common: &CamCommon) -> Result<i32> {
    let request = request_value(common)?;
    let answer = call!(common, vcad_cam_api::check_feeds_value, request);
    let ok = answer.get("ok") == Some(&Value::Bool(true));
    finish(
        common,
        &answer,
        check_feeds_summary,
        if ok { 0 } else { EXIT_REFUSED },
    )
}

fn check_feeds_summary(answer: &Value) {
    println!(
        "feeds against {}: {}",
        s(at(answer, "material"), "name"),
        if answer.get("ok") == Some(&Value::Bool(true)) {
            "usable"
        } else {
            "NOT usable as given"
        }
    );
    println!("  worst note: {}", s(answer, "worst_level"));
    print_notes(answer);
}

// ---------------------------------------------------------------------------
// gear
// ---------------------------------------------------------------------------

fn gear(common: &CamCommon) -> Result<i32> {
    let request = request_value(common)?;
    let answer = call!(common, vcad_cam_api::gear_value, request);
    // A reachability verdict that came back false is the answer being no: the
    // cutter cannot make a flank this gear needs.
    let mut unreachable: Vec<String> = Vec::new();
    for report in gear_reports(&answer) {
        let r = at(report, "reachability");
        if r.is_null() {
            continue;
        }
        if r.get("ok") != Some(&Value::Bool(true)) {
            unreachable.push(format!(
                "{}T: margin {:.6} mm, flank deviation {:.6} mm at the contact limit",
                report.get("teeth").and_then(Value::as_u64).unwrap_or(0),
                f(r, "margin"),
                f(report, "flank_deviation_at_contact_limit"),
            ));
        }
    }
    if !unreachable.is_empty() {
        eprintln!(
            "REFUSED: a Ø{:.3} cutter cannot cut every flank this gear needs — {}",
            f(&answer, "cutter_diameter"),
            unreachable.join("; ")
        );
    }
    let code = if unreachable.is_empty() {
        0
    } else {
        EXIT_REFUSED
    };
    finish(common, &answer, gear_summary, code)
}

/// Every gear report in the answer: the gear itself, then the train's members.
fn gear_reports(answer: &Value) -> Vec<&Value> {
    let mut out = Vec::new();
    let single = at(answer, "report");
    if !single.is_null() {
        out.push(single);
    }
    if let Some(list) = at(answer, "planetary.reports").as_array() {
        out.extend(list.iter());
    }
    out
}

fn gear_summary(answer: &Value) {
    let cutter = f(answer, "cutter_diameter");
    println!("gear, cut with a Ø{cutter:.3} mm end mill");
    for report in gear_reports(answer) {
        gear_report_summary(report);
    }
    if let Some(mesh) = at(answer, "planetary.mesh").as_object() {
        println!("\nplanetary train");
        for (k, v) in mesh {
            println!("  {k}: {v}");
        }
    }
    if let Some(c) = answer.get("compensation") {
        println!("\ncompensation from a measured reading");
        println!("  pin Ø{:.4} mm", f(c, "pin_diameter"));
        println!("  nominal  {:.4} mm", f(c, "nominal"));
        println!("  measured {:.4} mm", f(c, "measured"));
        let d = at(c, "detail");
        println!(
            "  tooth thickness error  {:+.4} mm",
            f(d, "thickness_error")
        );
        println!(
            "  move the cutter        {:+.4} mm along the flank normal",
            f(d, "tool_normal_offset")
        );
        println!(
            "                         {:+.4} mm radially",
            f(d, "tool_radial_offset")
        );
        println!("  {}", s(c, "note"));
    }
    if let Some(c) = answer.get("contours") {
        println!(
            "\ncontours for space {}: tooth space {} point(s), full profile {} point(s), tool centre path {} point(s)",
            c.get("tooth").and_then(Value::as_u64).unwrap_or(0),
            c.get("tooth_space").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
            c.get("full_profile").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
            c.get("tool_centre_path").and_then(Value::as_array).map(|a| a.len()).unwrap_or(0),
        );
    }
}

fn gear_report_summary(r: &Value) {
    println!(
        "\n  {}T module {:.3}, shift {:+.3}, {}",
        r.get("teeth").and_then(Value::as_u64).unwrap_or(0),
        f(r, "module"),
        f(r, "profile_shift"),
        if r.get("internal") == Some(&Value::Bool(true)) {
            "internal"
        } else {
            "external"
        },
    );
    println!(
        "    pitch r {:.4}   base r {:.4}   tip r {:.4} (nominal {:.4})",
        f(r, "pitch_radius"),
        f(r, "base_radius"),
        f(r, "tip_radius"),
        f(r, "nominal_tip_radius")
    );
    println!(
        "    root r {:.4} nominal → {:.4} as the cutter leaves it ({})",
        f(r, "nominal_root_radius"),
        f(r, "effective_root_radius"),
        s(r, "root_binding")
    );
    println!("    form (fillet tangency) r {:.4}", f(r, "form_radius"));
    println!(
        "    space width: {:.4} at the root, {:.4} at the form radius, {:.4} at the mouth",
        f(r, "space_width_at_root"),
        f(r, "space_width_at_form"),
        f(r, "mouth_width")
    );
    println!(
        "    tooth thickness {:.4} at pitch, tip land {:.4}",
        f(r, "pitch_tooth_thickness"),
        f(r, "tip_land")
    );
    let reach = at(r, "reachability");
    if !reach.is_null() {
        println!(
            "    reachability: {} — margin {:+.6} mm, flank deviation {:.6} mm at the contact limit, {}",
            if reach.get("ok") == Some(&Value::Bool(true)) { "OK" } else { "NOT REACHABLE" },
            f(reach, "margin"),
            f(r, "flank_deviation_at_contact_limit"),
            s(reach, "encroachment"),
        );
        match reach.get("flank_deviation_tolerance") {
            Some(Value::Null) | None => println!(
                "    graded strictly: the flank has to be exactly involute across contact."
            ),
            Some(t) => println!("    graded at a {} mm flank-deviation tolerance.", t),
        }
    }
    if let Some(p) = r.get("over_pins").filter(|v| !v.is_null()) {
        println!(
            "    over pins Ø{:.4}: M = {:.4} mm (pin centre r {:.4})",
            f(p, "pin_diameter"),
            f(p, "dimension"),
            f(p, "pin_centre_radius"),
        );
    }
    if let Some(p) = r.get("span").filter(|v| !v.is_null()) {
        println!(
            "    span over {} teeth: W = {:.4} mm (anvils touch at r {:.4})",
            p.get("teeth_spanned").and_then(Value::as_u64).unwrap_or(0),
            f(p, "length"),
            f(p, "contact_radius"),
        );
    }
}
