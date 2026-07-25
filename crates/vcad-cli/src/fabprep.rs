//! `vcad fab-prep` — routed board in, fab package plus DRC-delta receipt out.
//!
//! The command wrapper is deliberately thin: everything that decides anything
//! lives in [`vcad_ecad_fabprep`]. What belongs here is the part a CLI owes its
//! caller — a progress narration, and an exit code that means what it says.
//! A run that does not converge prints the offenders, writes the receipt and
//! the board, withholds every fabrication file, and exits non-zero.

use std::path::Path;

use anyhow::{Context, Result};
use vcad_ecad_fabprep::{package, render, run_fab_prep, FabPrepOptions, VerdictOptions};
use vcad_ir::ecad::{Pcb, PcbLayer};

/// Options as the CLI surface exposes them.
pub struct FabPrepArgs {
    /// Input `.pcb.json`.
    pub input: std::path::PathBuf,
    /// Output directory for the package.
    pub output: std::path::PathBuf,
    /// Derive and apply rule calibration (opt-in).
    pub calibrate_rules: bool,
    /// Skip the opening verdict pass.
    pub skip_routing: bool,
    /// Skip the dangling-copper prune.
    pub skip_prune: bool,
    /// Maximum fix-loop rounds.
    pub max_rounds: usize,
    /// Per-cluster search budget.
    pub budget: usize,
    /// Maximum connections per joint search window.
    pub max_cluster: usize,
    /// Write the receipt only — never fabrication files, even on success.
    pub report_only: bool,
    /// Skip the board SVG (the slowest optional artifact on a dense board).
    pub skip_svg: bool,
    /// DRC rules whose route-attributable violations are accepted rather than
    /// fixed. Logged in the receipt; an unknown name refuses the run.
    pub accept: Vec<String>,
}

/// Run the pipeline and write the package. Returns `Ok(false)` when the run did
/// not converge — the caller turns that into a non-zero exit.
pub fn run(args: &FabPrepArgs) -> Result<bool> {
    // Rounds are minutes apart on a dense board; narrate them by default.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .try_init();

    let text = std::fs::read_to_string(&args.input)
        .with_context(|| format!("reading {}", args.input.display()))?;
    let mut pcb: Pcb = serde_json::from_str(&text)
        .with_context(|| format!("parsing {} as a .pcb.json board", args.input.display()))?;

    println!(
        "fab-prep: {} — {} footprints, {} traces, {} vias",
        args.input.display(),
        pcb.footprints.len(),
        pcb.traces.len(),
        pcb.vias.len()
    );
    if args.calibrate_rules {
        println!("fab-prep: rule calibration ENABLED — every change is logged in the receipt");
    }
    if !args.accept.is_empty() {
        println!(
            "fab-prep: WAIVING {} — those violations are still counted and listed, they just \
             stop blocking the verdict",
            args.accept.join(", ")
        );
    }

    let opts = FabPrepOptions {
        calibrate_rules: args.calibrate_rules,
        route_remaining: !args.skip_routing,
        max_rounds: args.max_rounds,
        prune_dangling: !args.skip_prune,
        accept_rules: args.accept.clone(),
        verdict: VerdictOptions {
            budget: args.budget,
            max_cluster: args.max_cluster,
        },
    };
    let outcome = run_fab_prep(&mut pcb, &opts);

    // Narrate the verdict to stdout so a terminal run is readable without
    // opening the files. The full violation dump stays in drc_report.txt.
    print!("\n{}", render::summary(&outcome.report));

    if args.report_only {
        let written = package::write_report(&args.output, &pcb, &outcome)
            .context("writing the fab-prep report")?;
        println!(
            "\nfab-prep: report-only — wrote {} file(s) to {}",
            written.len(),
            args.output.display()
        );
        return Ok(outcome.report.converged);
    }

    // Copper-first inspection render: copper layers plus the outline, no value
    // or net labels. On a 479-footprint board the refdes text is a wall of
    // glyphs that buries the copper, and the copper is what a fab-prep reviewer
    // is looking at. Zone pours are dropped because rendering one runs a 2D
    // boolean per zone and dominates the run.
    let svg = (!args.skip_svg && outcome.report.converged).then(|| {
        let layers: Vec<PcbLayer> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .chain(std::iter::once(PcbLayer::EdgeCuts))
            .collect();
        let mut flat = pcb.clone();
        flat.zones.clear();
        vcad_render::pcb::render_pcb_svg_opts(
            &flat,
            &layers,
            12.0,
            &vcad_render::pcb::PcbRenderOpts {
                show_values: false,
                show_net_labels: false,
                ..Default::default()
            },
        )
    });

    match package::write_fab_package(&args.output, &pcb, &outcome, svg.as_deref()) {
        Ok(written) => {
            println!(
                "\nfab-prep: {}\nfab-prep: wrote {} file(s) to {}",
                outcome.report.headline(),
                written.len(),
                args.output.display()
            );
            Ok(true)
        }
        Err(package::PackageError::NotConverged(msg)) => {
            eprintln!("\nfab-prep: FAIL-CLOSED — {msg}");
            Ok(false)
        }
        Err(e) => Err(anyhow::Error::new(e).context("writing the fab package")),
    }
}

/// Default output directory next to the input when none is given.
pub fn default_output_dir(input: &Path) -> std::path::PathBuf {
    let stem = input
        .file_stem()
        .map(|s| s.to_string_lossy().replace(".pcb", ""))
        .unwrap_or_else(|| "board".into());
    input
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(format!("{stem}-fab"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_sits_beside_the_input() {
        assert_eq!(
            default_output_dir(Path::new("/tmp/cm5.pcb.json")),
            Path::new("/tmp/cm5-fab")
        );
        assert_eq!(
            default_output_dir(Path::new("board.json")),
            Path::new("board-fab")
        );
    }
}
