//! vcad CLI - Full-featured parametric CAD in the terminal
//!
//! Provides both an interactive TUI editor and headless commands for
//! creating and manipulating 3D CAD models.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

mod app;
mod chat_session;
mod fabprep;
mod input;
mod keybinding_adapter;
mod log_capture;
#[cfg(feature = "print-server")]
mod print_server;
mod raycast;
mod render;
mod repl;
mod tui;
mod ui;

#[derive(Parser)]
#[command(name = "vcad")]
#[command(about = "Full-featured parametric CAD in the terminal", long_about = None)]
#[command(version)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Do not read or write the on-disk cache of evaluated root meshes
    /// (`$VCAD_CACHE_DIR`, else `$XDG_CACHE_HOME/vcad`, else
    /// `~/.cache/vcad`; `VCAD_CACHE=0` has the same effect). `export` (STL,
    /// GLB) and `info` use the cache; it is keyed on each root's resolved
    /// expression plus the kernel build, so it never serves geometry from a
    /// different kernel or an edited root. STEP export needs the BRep and
    /// bypasses it regardless.
    #[arg(long, global = true)]
    no_cache: bool,
}

#[derive(Subcommand)]
enum Commands {
    /// Open the interactive TUI editor
    Tui {
        /// Path to a .vcad file to open
        file: Option<PathBuf>,
    },

    /// Interactive REPL for building geometry
    Repl {
        /// Optional file to load
        file: Option<PathBuf>,
    },

    /// Create a new vcad document
    New {
        /// Output file path
        file: PathBuf,
        /// Template: empty, cube, assembly
        #[arg(long, default_value = "empty")]
        template: String,
    },

    /// Export a .vcad (or .loon source) file to another format
    Export {
        /// Input .vcad or .loon file
        input: PathBuf,
        /// Output file (format determined by extension: .stl, .glb, .step, .stp, .urdf)
        output: PathBuf,
    },

    /// Import a STEP file to .vcad format
    Import {
        /// Input STEP file (.step or .stp)
        input: PathBuf,
        /// Output .vcad file
        output: PathBuf,
        /// Name for the imported part (default: derived from filename)
        #[arg(short, long)]
        name: Option<String>,
    },

    /// Import a URDF robot description file to .vcad format
    ImportUrdf {
        /// Input URDF file (.urdf or .xml)
        input: PathBuf,
        /// Output .vcad file
        output: PathBuf,
        /// Synthesize a floating (6-DOF) base when the URDF has none.
        ///
        /// Most humanoid/quadruped URDFs ship the world link and its
        /// `type="floating"` joint commented out, leaving the robot welded
        /// to the world on import. This injects them.
        #[arg(long)]
        floating_base: bool,
        /// Link to attach the floating base to (default: the tree's root).
        #[arg(long, value_name = "LINK", requires = "floating_base")]
        floating_base_link: Option<String>,
        /// Initial base height in mm for the synthesized floating base.
        /// Spawn just above the settled standing height (Booster K1: 620).
        #[arg(long, value_name = "MM")]
        spawn_height_mm: Option<f64>,
        /// Write mesh paths relative to the OUTPUT document instead of
        /// absolute — required for a document you intend to commit.
        ///
        /// The default absolute path resolves only on the machine that ran the
        /// import. Relative paths are resolved against the document's own
        /// directory when it is loaded from disk.
        #[arg(long)]
        relative_meshes: bool,
    },

    /// Recognise holes and bolt patterns in a STEP file
    ///
    /// Reads a vendor STEP, places the assembly, and reports each component's
    /// hole patterns — bolt-circle diameter, count, hole diameter, and the
    /// angle of every hole *relative to its pattern* — plus the body envelope
    /// measured from the largest coaxial cylinder rather than the bounding box.
    Features {
        /// Input STEP file (.step or .stp)
        input: PathBuf,
        /// Emit the full report as JSON instead of a text summary
        #[arg(long)]
        json: bool,
        /// Hide patterns with fewer than this many members (text output only)
        #[arg(long, default_value = "2", value_name = "N")]
        min_count: usize,
    },

    /// Render document to image
    Render {
        /// Input .vcad or .loon file
        input: PathBuf,
        /// Output image (PNG, JPEG)
        output: PathBuf,
        /// Image width in pixels
        #[arg(long, default_value = "1920")]
        width: u32,
        /// Image height in pixels
        #[arg(long, default_value = "1080")]
        height: u32,
        /// Camera azimuth angle (degrees)
        #[arg(long, default_value = "45")]
        azimuth: f64,
        /// Camera elevation angle (degrees)
        #[arg(long, default_value = "30")]
        elevation: f64,
        /// Camera distance (auto if not specified)
        #[arg(long)]
        distance: Option<f64>,
        /// Background color (hex, e.g. "1a1a2e" or "transparent")
        #[arg(long, default_value = "1a1a2e")]
        background: String,
        /// World up axis: z (kernel/CAD convention, default) or y
        #[arg(long, value_enum, default_value = "z")]
        up: UpAxisArg,
    },

    /// Apply boolean operation
    Boolean {
        /// Input .vcad or .loon file (loon requires --output)
        file: PathBuf,
        /// Operation: union, difference, intersection
        #[arg(value_enum)]
        op: BooleanOp,
        /// First part ID or name
        part_a: String,
        /// Second part ID or name
        part_b: String,
        /// Output file (default: modify in place)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Name for result part
        #[arg(long)]
        result_name: Option<String>,
    },

    /// Apply transform to part
    Transform {
        /// Input .vcad or .loon file (loon requires --output)
        file: PathBuf,
        /// Part ID or name
        part: String,
        /// Translation "x,y,z"
        #[arg(long)]
        translate: Option<String>,
        /// Rotation "rx,ry,rz" in degrees
        #[arg(long)]
        rotate: Option<String>,
        /// Scale "sx,sy,sz" or uniform "s"
        #[arg(long)]
        scale: Option<String>,
        /// Output file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },

    /// Semantic diff between two .vcad files (feature-level, id-matched)
    Diff {
        /// Old .vcad file
        a: PathBuf,
        /// New .vcad file
        b: PathBuf,
        /// Emit the structured diff as JSON instead of human-readable text
        #[arg(long)]
        json: bool,
        /// Exit with code 1 when the documents differ (like `git diff --exit-code`)
        #[arg(long)]
        exit_code: bool,
    },

    /// Three-way semantic merge of .vcad files (fail-closed on conflicts)
    Merge {
        /// Common-ancestor .vcad file
        base: PathBuf,
        /// Our side
        ours: PathBuf,
        /// Their side
        theirs: PathBuf,
        /// Output path for the merged document (default: overwrite `ours`,
        /// matching git merge-driver conventions)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Emit the conflict report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Display information about a .vcad (or .loon source) file
    Info {
        /// Path to the .vcad or .loon file
        file: PathBuf,
    },

    /// Resolve and compare variant parameter tables
    ///
    /// A design family — a full-size part and its scaled print mules — is one
    /// base table plus per-variant overlays, declared in a `.params.loon`
    /// file with `[deftable ...]` and `[defvariant ... :from ... :scale ...]`.
    /// `resolve` flattens one variant to concrete values, each carrying the
    /// source it came from; `diff` shows what two variants disagree on and
    /// whether the disagreement is an own override, an inherited value, or
    /// the envelope scale.
    Params {
        #[command(subcommand)]
        command: ParamsCommands,
    },

    /// Take a routed .pcb.json to a complete fab package plus a DRC-delta receipt
    ///
    /// Runs the whole fab-prep pipeline in one command: (optionally) calibrate
    /// the board's design rules from its own declared via classes, route or
    /// certify the remaining connections, then loop — census the violations the
    /// routing is answerable for, strip their nets, re-route through the
    /// session-probed ladder — until that number reaches zero. Prunes dangling
    /// copper, then exports Gerbers, drill, KiCad board, BOM and pick-and-place.
    ///
    /// The receipt reports route-attributable violations against the SAME board
    /// stripped of all routing, because absolute zero is not achievable on an
    /// imported fixture. If the loop does not converge, the offenders are
    /// reported, no fabrication files are written, and the exit code is 1.
    FabPrep {
        /// Routed board (.pcb.json)
        input: PathBuf,
        /// Output directory (default: <input>-fab next to the input)
        #[arg(short, long)]
        output: Option<PathBuf>,
        /// Derive and apply design-rule calibration from the board's own
        /// declared via classes and pre-existing holes. OFF by default:
        /// silently relaxing DRC rules to make a board pass is a footgun.
        /// Every change is logged with its derivation in the receipt.
        #[arg(long)]
        calibrate_rules: bool,
        /// Skip the opening verdict pass (take the board as fully routed)
        #[arg(long)]
        skip_routing: bool,
        /// Keep copper that reaches no pad or pour of its net
        #[arg(long)]
        skip_prune: bool,
        /// Skip the board SVG (the slowest optional artifact on a dense board)
        #[arg(long)]
        skip_svg: bool,
        /// Maximum strip-and-re-route rounds
        #[arg(long, default_value = "8")]
        max_rounds: usize,
        /// Per-cluster search budget (node expansions) for the complete router
        #[arg(long, default_value = "5000000")]
        budget: usize,
        /// Maximum connections coalesced into one joint search window
        #[arg(long, default_value = "6")]
        max_cluster: usize,
        /// Write the receipt and board only — never fabrication files
        #[arg(long)]
        report_only: bool,
        /// Accept a DRC rule's route-attributable violations instead of fixing
        /// them (repeatable, e.g. --accept MinTraceWidth). The violations are
        /// still counted and listed in the receipt and the waiver is named in
        /// FAB_NOTES.md — it stops blocking the verdict, it does not hide
        /// anything. An unrecognised rule name refuses the run.
        #[arg(long = "accept", value_name = "RULE")]
        accept: Vec<String>,
    },

    /// Run a physics simulation on a robot assembly (.vcad or .urdf)
    Simulate {
        /// Input file (.vcad, .loon, or .urdf). URDFs are imported in-memory.
        input: PathBuf,
        /// Number of simulation steps to run
        #[arg(long, default_value = "240")]
        steps: u32,
        /// Simulation timestep in seconds. Default is 1/240 s — the
        /// standard 240 Hz timestep used by phyz / Rapier / Bullet for
        /// stable rigid-body integration.
        #[arg(long, default_value_t = 1.0 / 240.0)]
        dt: f64,
        /// Print joint states every N steps (0 = only summary)
        #[arg(long, default_value = "60")]
        log_every: u32,
        /// Search root for `package://NAME/...` URI resolution. Repeat for
        /// multiple roots. Each root is expected to contain `NAME/`
        /// subdirectories (the standard ROS package layout).
        #[arg(long = "package-root", value_name = "DIR")]
        package_roots: Vec<PathBuf>,
        /// Synthesize a floating (6-DOF) base when a .urdf input has none.
        /// Without it the root link is grounded and the robot is welded to
        /// the world — it can neither walk nor fall.
        #[arg(long)]
        floating_base: bool,
        /// Link to attach the floating base to (default: the tree's root).
        #[arg(long, value_name = "LINK", requires = "floating_base")]
        floating_base_link: Option<String>,
        /// Initial base height in mm for `--floating-base`.
        #[arg(long, value_name = "MM")]
        spawn_height_mm: Option<f64>,
    },

    /// Slice a .vcad file for 3D printing
    Slice {
        /// Input .vcad or .loon file
        input: PathBuf,
        /// Output file (.gcode or .3mf)
        #[arg(short, long)]
        output: PathBuf,
        /// Printer profile
        #[arg(long, default_value = "generic")]
        profile: String,
        /// Layer height (mm)
        #[arg(long)]
        layer_height: Option<f64>,
        /// Wall count
        #[arg(long)]
        wall_count: Option<u32>,
        /// Infill density (0-100)
        #[arg(long)]
        infill: Option<u32>,
        /// Enable support
        #[arg(long)]
        support: bool,
        /// Print temperature (°C)
        #[arg(long)]
        print_temp: Option<u32>,
        /// Bed temperature (°C)
        #[arg(long)]
        bed_temp: Option<u32>,
        /// Use smart defaults from BRep analysis
        #[arg(long)]
        smart: bool,
        /// Print reasoning for smart defaults
        #[arg(long)]
        explain: bool,
    },

    /// Lint an EXPORTED mesh for FDM printability, in a chosen orientation
    ///
    /// Printability is a property of the shipped file, not of the model that
    /// produced it: a shell can pass its author's analytic profile check while
    /// the discretised STL carries 0.05 mm cracks. So this reads triangles and
    /// casts rays at them, reporting floating regions and mid-air islands,
    /// interior cracks, the overhang census, bridge spans with lengths, min
    /// wall against the nozzle, and a manifold + closed-sections summary.
    ///
    /// Exit code is 0 when clean and 1 when anything failed, so it gates CI.
    Check {
        /// Mesh to check (.stl, binary or ASCII)
        input: PathBuf,
        /// Which model axis points up on the plate: z, -z, x, -x, y, -y
        #[arg(long, default_value = "z")]
        orientation: String,
        /// Nozzle width in mm — nothing thinner can be extruded
        #[arg(long, default_value = "0.4")]
        nozzle: f64,
        /// Longest unsupported span accepted without support material, mm
        #[arg(long, default_value = "4")]
        max_bridge: f64,
        /// Material gaps below this are cracks, not channels. Never waived.
        #[arg(long, default_value = "0.15")]
        crack_threshold: f64,
        /// Self-support limit in degrees from vertical
        #[arg(long, default_value = "45")]
        max_overhang: f64,
        /// Fail on the overhang census instead of warning
        #[arg(long)]
        strict_overhangs: bool,
        /// Distance between raycast columns, mm (default: the nozzle width)
        #[arg(long)]
        pitch: Option<f64>,
        /// Section sampling pitch in mm
        #[arg(long, default_value = "0.4")]
        section_step: f64,
        /// Accept unsupported spans in a height range, e.g. `--allow-bridge
        /// 1.75:2.65`. Repeatable. Documented bridges only — a crack is never
        /// waived by this.
        #[arg(long = "allow-bridge", value_name = "Z0:Z1")]
        allow_bridge: Vec<String>,
        /// Emit the report as JSON
        #[arg(long)]
        json: bool,
    },

    /// Start the print relay server for web app → printer communication
    #[cfg(feature = "print-server")]
    PrintServer {
        /// Port to listen on
        #[arg(long, default_value = "7878")]
        port: u16,
    },

    /// Run a probe suite: material/void and clearance assertions against a
    /// posed assembly, with a nonzero exit code on any failure.
    ///
    /// The suite is a JSON file listing named parts (mesh + pose) and the
    /// assertions made against them; mesh paths resolve relative to it. See
    /// `vcad_kernel_tessellate::probe` for the schema.
    Probe {
        /// Path to the probe suite JSON.
        file: PathBuf,

        /// Print only the failures and the tally.
        #[arg(long)]
        quiet: bool,
    },

    /// Sign in so the chat panel uses your account's quota instead of
    /// the anonymous limit. Without `--token` this opens a browser
    /// and polls for the device-code flow to complete.
    Login {
        /// Paste-token path: write the JWT directly without opening a browser.
        #[arg(long)]
        token: Option<String>,
    },

    /// Remove the stored chat auth token.
    Logout,
}

#[derive(Subcommand)]
enum ParamsCommands {
    /// Flatten one variant to a resolved table (JSON on stdout)
    Resolve {
        /// Variant (or base table) name
        variant: String,
        /// Parameter-table file (default: `params.loon` in the working
        /// directory, or $VCAD_PARAMS)
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Print an aligned table instead of JSON
        #[arg(long)]
        table: bool,
    },

    /// Show what differs between two variants, and why
    Diff {
        /// Left variant
        a: String,
        /// Right variant
        b: String,
        /// Parameter-table file (default: `params.loon` in the working
        /// directory, or $VCAD_PARAMS)
        #[arg(short, long)]
        file: Option<PathBuf>,
        /// Emit JSON instead of the human-readable rendering
        #[arg(long)]
        json: bool,
        /// Exit 1 when the variants differ (for CI gates)
        #[arg(long)]
        exit_code: bool,
    },

    /// List the tables and variants a parameter-table file declares
    List {
        /// Parameter-table file (default: `params.loon` in the working
        /// directory, or $VCAD_PARAMS)
        #[arg(short, long)]
        file: Option<PathBuf>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

/// World up axis for `vcad render`.
#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum UpAxisArg {
    /// +Z up — matches the kernel, loon semantics and the docs.
    Z,
    /// +Y up — for meshes authored in a Y-up (graphics) frame.
    Y,
}

impl From<UpAxisArg> for crate::render::UpAxis {
    fn from(a: UpAxisArg) -> Self {
        match a {
            UpAxisArg::Z => crate::render::UpAxis::Z,
            UpAxisArg::Y => crate::render::UpAxis::Y,
        }
    }
}

fn main() -> Result<()> {
    vcad_i18n::init(&vcad_i18n::Locale::from_env());
    let cli = Cli::parse();
    if cli.no_cache {
        // One switch for every evaluation below, however deep; the cache
        // reads this at construction (`DiskMeshCache::from_env`).
        std::env::set_var("VCAD_CACHE", "0");
    }

    match cli.command {
        Some(Commands::Tui { file }) => {
            app::run_tui(file)?;
        }
        Some(Commands::Repl { file }) => {
            repl::run_repl(file)?;
        }
        Some(Commands::New { file, template }) => {
            create_new(&file, &template)?;
        }
        Some(Commands::Export { input, output }) => {
            export_file(&input, &output)?;
        }
        Some(Commands::Import {
            input,
            output,
            name,
        }) => {
            import_step(&input, &output, name)?;
        }
        Some(Commands::ImportUrdf {
            input,
            output,
            floating_base,
            floating_base_link,
            spawn_height_mm,
            relative_meshes,
        }) => {
            import_urdf(
                &input,
                &output,
                FloatingBase {
                    enabled: floating_base,
                    link: floating_base_link,
                    spawn_height_mm,
                },
                relative_meshes,
            )?;
        }
        Some(Commands::Features {
            input,
            json,
            min_count,
        }) => {
            recognize_features(&input, json, min_count)?;
        }
        Some(Commands::Render {
            input,
            output,
            width,
            height,
            azimuth,
            elevation,
            distance,
            background,
            up,
        }) => {
            render_to_image(
                &input,
                &output,
                width,
                height,
                azimuth,
                elevation,
                distance,
                &background,
                up.into(),
            )?;
        }
        Some(Commands::Boolean {
            file,
            op,
            part_a,
            part_b,
            output,
            result_name,
        }) => {
            apply_boolean(&file, op, &part_a, &part_b, output.as_ref(), result_name)?;
        }
        Some(Commands::Transform {
            file,
            part,
            translate,
            rotate,
            scale,
            output,
        }) => {
            apply_transform(&file, &part, translate, rotate, scale, output.as_ref())?;
        }
        Some(Commands::Diff {
            a,
            b,
            json,
            exit_code,
        }) => {
            run_diff(&a, &b, json, exit_code)?;
        }
        Some(Commands::Merge {
            base,
            ours,
            theirs,
            output,
            json,
        }) => {
            run_merge(&base, &ours, &theirs, output.as_deref(), json)?;
        }
        Some(Commands::Info { file }) => {
            show_info(&file)?;
        }
        Some(Commands::Params { command }) => {
            run_params(command)?;
        }
        Some(Commands::FabPrep {
            input,
            output,
            calibrate_rules,
            skip_routing,
            skip_prune,
            skip_svg,
            max_rounds,
            budget,
            max_cluster,
            report_only,
            accept,
        }) => {
            let output = output.unwrap_or_else(|| fabprep::default_output_dir(&input));
            let converged = fabprep::run(&fabprep::FabPrepArgs {
                input,
                output,
                calibrate_rules,
                skip_routing,
                skip_prune,
                skip_svg,
                max_rounds,
                budget,
                max_cluster,
                report_only,
                accept,
            })?;
            // Fail closed: a board that did not converge must not look like a
            // successful run to a script or a CI step.
            if !converged {
                std::process::exit(1);
            }
        }
        Some(Commands::Simulate {
            input,
            steps,
            dt,
            log_every,
            package_roots,
            floating_base,
            floating_base_link,
            spawn_height_mm,
        }) => {
            simulate_file(
                &input,
                steps,
                dt,
                log_every,
                &package_roots,
                FloatingBase {
                    enabled: floating_base,
                    link: floating_base_link,
                    spawn_height_mm,
                },
            )?;
        }
        Some(Commands::Slice {
            input,
            output,
            profile,
            layer_height,
            wall_count,
            infill,
            support,
            print_temp,
            bed_temp,
            smart,
            explain,
        }) => {
            slice_file(
                &input,
                &output,
                &profile,
                layer_height,
                wall_count,
                infill,
                support,
                print_temp,
                bed_temp,
                smart,
                explain,
            )?;
        }
        Some(Commands::Check {
            input,
            orientation,
            nozzle,
            max_bridge,
            crack_threshold,
            max_overhang,
            strict_overhangs,
            pitch,
            section_step,
            allow_bridge,
            json,
        }) => {
            run_check(
                &input,
                &orientation,
                nozzle,
                max_bridge,
                crack_threshold,
                max_overhang,
                strict_overhangs,
                pitch,
                section_step,
                &allow_bridge,
                json,
            )?;
        }
        #[cfg(feature = "print-server")]
        Some(Commands::PrintServer { port }) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(print_server::start_server(port))?;
        }
        Some(Commands::Probe { file, quiet }) => {
            run_probe(&file, quiet)?;
        }
        Some(Commands::Login { token }) => {
            run_login(token)?;
        }
        Some(Commands::Logout) => {
            run_logout()?;
        }
        None => {
            // Default to TUI with no file
            app::run_tui(None)?;
        }
    }

    Ok(())
}

/// `vcad probe` — run a probe suite and gate CI on the result.
///
/// Exits with status 1 when any assertion fails, so a suite can sit in a
/// build pipeline the way rana ran `probe-60c.py` by hand.
fn run_probe(file: &std::path::Path, quiet: bool) -> Result<()> {
    let report =
        vcad_kernel_tessellate::run_probe_file(file).map_err(|e| anyhow::anyhow!("{e}"))?;
    for outcome in &report.outcomes {
        if quiet && outcome.passed {
            continue;
        }
        println!(
            "  {} {}: {}",
            if outcome.passed { "PASS" } else { "FAIL" },
            outcome.name,
            outcome.detail
        );
    }
    println!(
        "{} passed, {} failed, {} total",
        report.passed(),
        report.failed(),
        report.outcomes.len()
    );
    if !report.ok() {
        std::process::exit(1);
    }
    Ok(())
}

/// `vcad login` — accept a pasted JWT or drive the device-code flow.
fn run_login(token: Option<String>) -> Result<()> {
    if let Some(jwt) = token {
        let jwt = jwt.trim().to_string();
        if jwt.is_empty() {
            anyhow::bail!("--token must not be empty");
        }
        vcad_chat::save_token(&vcad_chat::Token {
            access_token: jwt,
            refresh_token: None,
            expires_at: None,
        })?;
        let path = vcad_chat::token_path()?;
        println!("Saved token to {}", path.display());
        return Ok(());
    }

    // Device-code browser flow.
    let device = vcad_chat::generate_device_code();
    println!("Opening {}", device.login_url);
    println!("(if your browser didn't open, visit the URL above and sign in)");
    if let Err(e) = vcad_chat::open_browser(&device.login_url) {
        eprintln!("note: couldn't spawn a browser ({e}); open the URL manually");
    }

    println!("Waiting for sign-in to complete…");
    match vcad_chat::poll_for_token(&device.code, None) {
        Ok(token) => {
            vcad_chat::save_token(&token)?;
            let path = vcad_chat::token_path()?;
            println!("Saved token to {}", path.display());
            Ok(())
        }
        Err(e) => {
            anyhow::bail!("device-code login failed: {e}")
        }
    }
}

/// `vcad logout` — remove the stored token.
fn run_logout() -> Result<()> {
    vcad_chat::clear_token()?;
    println!("Logged out — chat will use anonymous quota.");
    Ok(())
}

/// Does this path name loon source rather than `.vcad` IR? Loon inputs are
/// evaluated on the way in, so the CLI works on source, not build artifacts.
fn is_loon(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("loon")
}

/// Evaluate a `.loon` file to a document, resolving `[use ...]` module
/// imports against the file's own directory.
fn eval_loon(path: &std::path::Path) -> Result<vcad_ir::Document> {
    vcad_loon::eval_vcad_file(path).map_err(|e| anyhow::anyhow!("{e}"))
}

fn load_doc(path: &std::path::Path) -> Result<vcad_ir::Document> {
    if is_loon(path) {
        return eval_loon(path);
    }
    let json = std::fs::read_to_string(path)
        .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", path.display()))?;
    let mut doc = vcad_ir::Document::from_json(&json)
        .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", path.display()))?;
    // Mesh references are opened verbatim during evaluation, so a document
    // written with `import-urdf --relative-meshes` — the mode that makes a
    // document committable — resolves only when the CLI happens to be run
    // from the right directory, and otherwise silently evaluates to nothing.
    // Anchor them to the document's own location now that it is known.
    if let Some(dir) = path.parent() {
        vcad_eval::resolve_mesh_paths(&mut doc, dir);
    }
    Ok(doc)
}

fn run_diff(a: &std::path::Path, b: &std::path::Path, json: bool, exit_code: bool) -> Result<()> {
    let doc_a = load_doc(a)?;
    let doc_b = load_doc(b)?;
    let d = vcad_diff::diff(&doc_a, &doc_b)?;
    if json {
        println!("{}", serde_json::to_string_pretty(&d)?);
    } else {
        print!("{}", vcad_diff::render_human(&d));
    }
    if exit_code && !d.is_empty() {
        std::process::exit(1);
    }
    Ok(())
}

fn run_merge(
    base: &std::path::Path,
    ours: &std::path::Path,
    theirs: &std::path::Path,
    output: Option<&std::path::Path>,
    json: bool,
) -> Result<()> {
    let doc_base = load_doc(base)?;
    let doc_ours = load_doc(ours)?;
    let doc_theirs = load_doc(theirs)?;
    match vcad_diff::merge(&doc_base, &doc_ours, &doc_theirs)? {
        vcad_diff::MergeResult::Merged(merged) => {
            let out = output.unwrap_or(ours);
            std::fs::write(out, merged.to_json()?)?;
            println!("merged cleanly \u{2192} {}", out.display());
            Ok(())
        }
        vcad_diff::MergeResult::Conflicts(conflicts) => {
            if json {
                eprintln!("{}", serde_json::to_string_pretty(&conflicts)?);
            } else {
                eprint!("{}", vcad_diff::render_conflicts(&conflicts));
            }
            std::process::exit(1);
        }
    }
}

fn export_file(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use std::fs;

    let doc = load_vcad_document(input)?;

    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("loon") {
        return export_loon(&doc, output);
    }

    // Evaluate document to get meshes (not needed for loon)
    let meshes = crate::app::evaluate_document(&doc)?;

    match ext.to_lowercase().as_str() {
        "stl" => {
            // Combine all meshes and export as STL
            let mut combined_verts = Vec::new();
            let mut combined_idxs = Vec::new();
            for mesh in &meshes {
                let base_idx = (combined_verts.len() / 3) as u32;
                combined_verts.extend_from_slice(&mesh.vertices);
                for idx in &mesh.indices {
                    combined_idxs.push(idx + base_idx);
                }
            }
            warn_floating_floors(&combined_verts, &combined_idxs);
            let stl_bytes = export_stl_bytes(&combined_verts, &combined_idxs)?;
            fs::write(output, stl_bytes)?;
            println!("Exported STL to {}", output.display());
        }
        "glb" => {
            let count = write_glb(&doc, output)?;
            println!(
                "Exported GLB ({count} mesh{}) to {}",
                if count == 1 { "" } else { "es" },
                output.display()
            );
        }
        "step" | "stp" => {
            export_step(&doc, output)?;
        }
        "urdf" => {
            export_urdf(&doc, output)?;
        }
        _ => {
            anyhow::bail!("Unknown output format: {}", ext);
        }
    }

    Ok(())
}

fn export_loon(doc: &vcad_ir::Document, output: &PathBuf) -> Result<()> {
    use std::fs;

    let (source, unsupported) = vcad_ir::to_loon::document_to_loon_checked(doc);
    fs::write(output, &source)?;

    if unsupported.is_empty() {
        println!("Exported loon to {}", output.display());
    } else {
        eprintln!(
            "warning: loon export of '{}' dropped unsupported variants: {}",
            output.display(),
            unsupported.join(", ")
        );
        eprintln!(
            "         These nodes were replaced with comment placeholders and will not round-trip."
        );
        anyhow::bail!(
            "loon export incomplete — {} unsupported variant(s): {}",
            unsupported.len(),
            unsupported.join(", ")
        );
    }

    Ok(())
}

/// The `floating_floor` DFM check (vcad.dfm/1, FDM pack): a flat
/// downward-facing triangle above first-layer height with NO geometry
/// below its footprint starts printing in mid-air — the slicer's
/// "floating cantilever", caught at export instead of in someone else's
/// tool.
///
/// Print orientation is the user's choice, not the model frame's, so the
/// check runs in all six axis-down candidates and reports which ones are
/// support-free. It only warns; supports are sometimes the plan.
fn warn_floating_floors(vertices: &[f32], indices: &[u32]) {
    // The support scan below is O(floors × triangles) per orientation. Past
    // this size it costs minutes (a 108-plate sheet-metal robot: ~780k
    // triangles, ~10 min) for a 3D-printing hint that rarely applies to a
    // mesh that large. Say so and skip, rather than stall the export.
    const MAX_TRIANGLES: usize = 200_000;
    let tri_count = indices.len() / 3;
    if tri_count > MAX_TRIANGLES {
        eprintln!(
            "note[floating_floor]: print-orientation check skipped ({tri_count} triangles > \
             {MAX_TRIANGLES}); check the part you intend to print on its own"
        );
        return;
    }
    // (label, index of the "up" coordinate, sign) — "down" = -sign axis.
    const ORIENTS: [(&str, usize, f64); 6] = [
        ("+Z up", 2, 1.0),
        ("-Z up", 2, -1.0),
        ("+Y up", 1, 1.0),
        ("-Y up", 1, -1.0),
        ("+X up", 0, 1.0),
        ("-X up", 0, -1.0),
    ];
    // Below this, hanging floors are bridge-scale slivers, not shelves.
    const MIN_AREA_MM2: f64 = 3.0;
    let mut clean: Vec<&str> = Vec::new();
    let mut counts: Vec<(&str, f64)> = Vec::new();
    for (label, up, sign) in ORIENTS {
        let a = floating_floor_area(vertices, indices, up, sign);
        if a < MIN_AREA_MM2 {
            clean.push(label);
        }
        counts.push((label, a));
    }
    if clean.len() == ORIENTS.len() {
        return; // clean every way up: nothing to say
    }
    if clean.is_empty() {
        eprintln!(
            "warning[floating_floor]: no support-free print orientation — \
             every axis-down choice leaves downward faces starting in \
             mid-air ({}). Re-orient a feature to a face, or plan supports.",
            counts
                .iter()
                .map(|(l, a)| format!("{l}: {a:.0} mm2"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    } else {
        eprintln!(
            "note[floating_floor]: support-free print orientation(s): {}. \
             Other orientations have floating floors ({}).",
            clean.join(", "),
            counts
                .iter()
                .filter(|(_, a)| *a >= MIN_AREA_MM2)
                .map(|(l, a)| format!("{l}: {a:.0} mm2"))
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}

/// Total mid-air floor AREA (mm²) with `axes[up]`·`sign` as world up.
///
/// Area, not count: seam tessellation leaves sub-nozzle sliver ledges
/// (measured: a 0.18 mm annulus where a sphere hands off to a bore — 21
/// facets, bridges trivially). A latch shelf that actually fails is
/// hundreds of mm². The rule's threshold separates them.
fn floating_floor_area(vertices: &[f32], indices: &[u32], up: usize, sign: f64) -> f64 {
    let v = |k: u32| {
        let i = (k as usize) * 3;
        let p = [
            vertices[i] as f64,
            vertices[i + 1] as f64,
            vertices[i + 2] as f64,
        ];
        // Rotate the chosen axis into +Z: z' = sign * p[up], and keep the
        // other two as the build-plane coordinates.
        let (a, b) = match up {
            0 => (p[1], p[2]),
            1 => (p[0], p[2]),
            _ => (p[0], p[1]),
        };
        [a, b, sign * p[up]]
    };
    let mut zmin = f64::INFINITY;
    let mut tris: Vec<([f64; 3], [f64; 3], [f64; 3])> = Vec::new();
    let mut floors: Vec<([f64; 3], f64)> = Vec::new();
    for t in indices.as_chunks::<3>().0 {
        let (a, b, c) = (v(t[0]), v(t[1]), v(t[2]));
        for p in [&a, &b, &c] {
            zmin = zmin.min(p[2]);
        }
        tris.push((a, b, c));
    }
    for (a, b, c) in &tris {
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let w = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            u[1] * w[2] - u[2] * w[1],
            u[2] * w[0] - u[0] * w[2],
            u[0] * w[1] - u[1] * w[0],
        ];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if l < 1e-12 {
            continue;
        }
        // Winding orientation is not trusted here (export accepts meshes
        // from several producers): treat near-horizontal faces as floors
        // by |nz|, then let the support scan below decide.
        if (n[2] / l).abs() < 0.985 {
            continue;
        }
        let mid = [
            (a[0] + b[0] + c[0]) / 3.0,
            (a[1] + b[1] + c[1]) / 3.0,
            (a[2] + b[2] + c[2]) / 3.0,
        ];
        if mid[2] > zmin + 0.5 {
            floors.push((mid, l * 0.5));
        }
    }
    let mut hanging = 0.0f64;
    for (mid, area) in &floors {
        let mut support_below = false;
        let mut solid_below = false;
        for (a, b, c) in &tris {
            let xmin = a[0].min(b[0]).min(c[0]) - 1e-6;
            let xmax = a[0].max(b[0]).max(c[0]) + 1e-6;
            let ymin = a[1].min(b[1]).min(c[1]) - 1e-6;
            let ymax = a[1].max(b[1]).max(c[1]) + 1e-6;
            if mid[0] < xmin || mid[0] > xmax || mid[1] < ymin || mid[1] > ymax {
                continue;
            }
            let ztop = a[2].max(b[2]).max(c[2]);
            if ztop < mid[2] - 1e-4 {
                support_below = true;
                break;
            }
            let zbot = a[2].min(b[2]).min(c[2]);
            if zbot < mid[2] - 1e-4 {
                solid_below = true;
            }
        }
        // A "floor" with same-solid geometry continuing beneath it is a
        // ceiling seen from inside a cavity — not a floor at all.
        if !support_below && !solid_below {
            hanging += area;
        }
    }
    hanging
}

fn export_stl_bytes(vertices: &[f32], indices: &[u32]) -> Result<Vec<u8>> {
    let num_triangles = indices.len() / 3;
    let mut data = Vec::with_capacity(84 + num_triangles * 50);

    // 80-byte header
    data.extend_from_slice(
        b"vcad-cli STL export                                                             ",
    );
    // Number of triangles
    data.extend_from_slice(&(num_triangles as u32).to_le_bytes());

    for tri in indices.chunks(3) {
        let i0 = tri[0] as usize * 3;
        let i1 = tri[1] as usize * 3;
        let i2 = tri[2] as usize * 3;

        let v0 = [vertices[i0], vertices[i0 + 1], vertices[i0 + 2]];
        let v1 = [vertices[i1], vertices[i1 + 1], vertices[i1 + 2]];
        let v2 = [vertices[i2], vertices[i2 + 1], vertices[i2 + 2]];

        // Compute normal
        let e1 = [v1[0] - v0[0], v1[1] - v0[1], v1[2] - v0[2]];
        let e2 = [v2[0] - v0[0], v2[1] - v0[1], v2[2] - v0[2]];
        let nx = e1[1] * e2[2] - e1[2] * e2[1];
        let ny = e1[2] * e2[0] - e1[0] * e2[2];
        let nz = e1[0] * e2[1] - e1[1] * e2[0];
        let len = (nx * nx + ny * ny + nz * nz).sqrt();
        let (nx, ny, nz) = if len > 1e-10 {
            (nx / len, ny / len, nz / len)
        } else {
            (0.0, 0.0, 1.0)
        };

        // Normal
        data.extend_from_slice(&nx.to_le_bytes());
        data.extend_from_slice(&ny.to_le_bytes());
        data.extend_from_slice(&nz.to_le_bytes());
        // Vertices
        for v in [v0, v1, v2] {
            data.extend_from_slice(&v[0].to_le_bytes());
            data.extend_from_slice(&v[1].to_le_bytes());
            data.extend_from_slice(&v[2].to_le_bytes());
        }
        // Attribute byte count
        data.extend_from_slice(&0u16.to_le_bytes());
    }

    Ok(data)
}

/// Write the evaluated document to a binary GLB at `output`. Returns the
/// number of meshes written. Print-free so the TUI can surface the result
/// in its status line.
fn write_glb(doc: &vcad_ir::Document, output: &PathBuf) -> Result<usize> {
    use vcad_kernel_export::{build_glb, GlbMeshSpec, GlbSpec};

    let scene = crate::app::evaluate_scene(doc)?;

    let mut f32_data: Vec<f32> = Vec::new();
    let mut u32_data: Vec<u32> = Vec::new();
    let mut meshes = Vec::new();
    for (i, part) in scene.parts.iter().enumerate() {
        if part.mesh.positions.is_empty() || part.mesh.indices.is_empty() {
            continue;
        }
        let pos_off = f32_data.len();
        f32_data.extend_from_slice(&part.mesh.positions);
        let normals = part.mesh.normals.as_ref().map(|n| {
            let off = f32_data.len();
            f32_data.extend_from_slice(n);
            [off, n.len()]
        });
        let idx_off = u32_data.len();
        u32_data.extend_from_slice(&part.mesh.indices);
        let name = doc
            .roots
            .get(i)
            .and_then(|r| doc.nodes.get(&r.root))
            .and_then(|n| n.name.clone())
            .unwrap_or_else(|| format!("part_{}", i + 1));
        meshes.push(GlbMeshSpec {
            name,
            positions: [pos_off, part.mesh.positions.len()],
            indices: [idx_off, part.mesh.indices.len()],
            normals,
            color: [0.71, 0.71, 0.75],
            metallic: 0.1,
            roughness: 0.7,
            emissive: None,
            emissive_strength: None,
            clearcoat: None,
            clearcoat_roughness: None,
            alpha: None,
            transform: None,
            mesh_key: None,
        });
    }
    if meshes.is_empty() {
        anyhow::bail!("Document has no geometry to export");
    }

    let count = meshes.len();
    let scene_name = output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("vcad")
        .to_string();
    let glb = build_glb(
        &GlbSpec {
            name: scene_name,
            meshes,
            animation: None,
            scene_extras: None,
        },
        &f32_data,
        &u32_data,
    )
    .map_err(|e| anyhow::anyhow!("GLB build failed: {e}"))?;
    std::fs::write(output, glb)?;
    Ok(count)
}

fn export_step(doc: &vcad_ir::Document, output: &PathBuf) -> Result<()> {
    let count = write_step(doc, output)?;
    println!(
        "Exported STEP ({} solid{}) to {}",
        count,
        if count == 1 { "" } else { "s" },
        output.display()
    );
    Ok(())
}

/// Write the evaluated document to AP214 STEP at `output`. Returns the number
/// of solids written. Mesh-only roots are refused by name (existing policy);
/// print-free so the TUI can surface the error in its status line.
fn write_step(doc: &vcad_ir::Document, output: &PathBuf) -> Result<usize> {
    use vcad_kernel::Solid;

    // Evaluate the document through the kernel: booleans, transforms,
    // fillets, sweeps etc. all preserve BRep, so the result serializes to
    // AP214 with true analytic faces.
    let roots = vcad_eval::evaluate_root_solids(doc)
        .map_err(|e| anyhow::anyhow!("failed to evaluate document: {e}"))?;

    if roots.is_empty() {
        anyhow::bail!("Document has no geometry to export");
    }

    // Refuse per-root, naming the offenders, instead of a blanket refusal.
    let mesh_only: Vec<String> = roots
        .iter()
        .filter(|r| !r.solid.as_ref().is_some_and(Solid::can_export_step))
        .map(|r| {
            let op = doc
                .nodes
                .get(&r.node_id)
                .and_then(|n| serde_json::to_value(&n.op).ok())
                .and_then(|v| v.get("type").and_then(|t| t.as_str().map(String::from)))
                .unwrap_or_else(|| "unknown op".to_string());
            match &r.name {
                Some(name) => format!("'{}' (node {}, {})", name, r.node_id, op),
                None => format!("node {} ({})", r.node_id, op),
            }
        })
        .collect();
    if !mesh_only.is_empty() {
        anyhow::bail!(
            "STEP export requires BRep geometry, but {} of {} root(s) evaluated to \
             mesh-only or empty solids: {}. These ops (e.g. mesh imports, degraded \
             booleans) have no analytic faces to serialize — export those parts as \
             STL, or fix the failing feature.",
            mesh_only.len(),
            roots.len(),
            mesh_only.join(", ")
        );
    }

    let named: Vec<(&Solid, String)> = roots
        .iter()
        .enumerate()
        .filter_map(|(i, r)| {
            let name = r.name.clone().unwrap_or_else(|| format!("part_{}", i + 1));
            r.solid.as_ref().map(|s| (s, name))
        })
        .collect();
    let refs: Vec<(&Solid, &str)> = named.iter().map(|(s, n)| (*s, n.as_str())).collect();
    let buffer = Solid::solids_to_step_buffer(&refs)?;
    std::fs::write(output, buffer)?;
    // Count what was actually serialized rather than what was evaluated, so
    // the caller's message can't drift from the file if the guard above ever
    // stops rejecting every solid-less root.
    Ok(named.len())
}

/// Recognise holes and bolt patterns in a STEP file and print them.
///
/// The text form is written for someone about to design a mating part: what
/// the pattern is, how big, and where each hole sits *relative to the pattern*
/// — absolute angles depend on how the vendor happened to place the model and
/// make the same part look different in two files.
fn recognize_features(input: &PathBuf, json: bool, min_count: usize) -> Result<()> {
    use vcad_kernel_features::{PatternKind, PatternMember};

    let report = vcad_kernel_features::step::recognize_step_file(input)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    let env = report.envelope();
    let axis = env.dominant_axis;
    println!("{}", input.display());
    println!(
        "  axis            ({:.3}, {:.3}, {:.3})",
        axis.x, axis.y, axis.z
    );
    match env.body_od_mm {
        Some(od) => println!(
            "  body OD         {od:.3} mm  (largest coaxial cylinder; bbox across the axis reads {:.3})",
            env.bbox_across_axis_mm
        ),
        None => println!("  body OD         n/a (no coaxial cylinder)"),
    }
    println!("  axial length    {:.3} mm", env.axial_length_mm);
    println!("  components      {}", report.components.len());

    for component in &report.components {
        let patterns: Vec<_> = component
            .report
            .patterns
            .iter()
            .filter(|p| p.count >= min_count)
            .collect();
        if patterns.is_empty() {
            continue;
        }
        println!("\n{} ({} faces)", component.name, component.face_count);
        for (idx, p) in component.report.patterns.iter().enumerate() {
            if p.count < min_count {
                continue;
            }
            println!("  [{idx}] {}", p.describe());
            if let PatternKind::BoltCircle { .. } = p.kind {
                let angles: Vec<String> = p
                    .members
                    .iter()
                    .map(|m: &PatternMember| format!("{:.3}", m.angle_deg))
                    .collect();
                println!("       angles rel. to pattern: {}", angles.join(", "));
                if let Some(abs) = p.first_member_absolute_deg {
                    println!("       first hole at {abs:.3} deg absolute (placement-dependent)");
                }
            }
            let axial = p
                .members
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |a, m| {
                    (a.0.min(m.axial_start), a.1.max(m.axial_end))
                });
            println!(
                "       axial extent along the pattern axis: [{:.3}, {:.3}]",
                axial.0, axial.1
            );
        }
        for rel in &component.report.relations {
            if !rel.bisects_adjacent {
                continue;
            }
            println!(
                "  [{}] bisects adjacent holes of [{}] (phase {:.3} deg)",
                rel.subject, rel.reference, rel.phase_deg
            );
        }
    }
    Ok(())
}

fn import_step(input: &PathBuf, output: &PathBuf, name: Option<String>) -> Result<()> {
    use std::fs;
    use vcad_kernel::Solid;

    // Derive name from filename if not provided
    let part_name = name.unwrap_or_else(|| {
        input
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("imported")
            .to_string()
    });

    // Import the STEP file
    let (solids, report) = Solid::from_step_all_with_report(input)?;

    if let Some(summary) = report.summary() {
        eprintln!(
            "warning: STEP import skipped {} face(s) with unsupported surface types:",
            report.total_skipped_faces()
        );
        eprintln!("{summary}");
    }

    if solids.is_empty() {
        anyhow::bail!("No solids found in STEP file");
    }

    // Create a vcad document
    let mut doc = vcad_ir::Document::new();

    for (i, _solid) in solids.iter().enumerate() {
        let node_name = if solids.len() == 1 {
            part_name.clone()
        } else {
            format!("{}_{}", part_name, i)
        };

        let node_id = (i + 1) as u64;
        doc.nodes.insert(
            node_id,
            vcad_ir::Node {
                id: node_id,
                name: Some(node_name),
                op: vcad_ir::CsgOp::StepImport {
                    path: input.to_string_lossy().into_owned(),
                    // Each node names its own body. Before `solid_index`
                    // existed every node here resolved to solid 0, so a
                    // multi-body STEP became N copies of the first body.
                    solid_index: if i == 0 { None } else { Some(i as u32) },
                },
            },
        );
        doc.roots.push(vcad_ir::SceneEntry {
            root: node_id,
            material: "default".to_string(),
            visible: None,
        });
    }

    // Write the document
    let json = doc.to_json()?;
    fs::write(output, json)?;

    println!(
        "Imported {} solid(s) from {} to {}",
        solids.len(),
        input.display(),
        output.display()
    );
    Ok(())
}

/// Read a .vcad file and return the materialized IR document.
/// Auto-detects CRDT (v0.4) vs legacy v1 JSON shapes. A `.loon` input is
/// evaluated to a document first.
fn load_vcad_document(file: &PathBuf) -> Result<vcad_ir::Document> {
    let mut doc = load_vcad_document_raw(file)?;
    // Anchor relative mesh references to the document's own directory — see
    // the note in `load_doc`. Without this, a committed URDF import evaluates
    // to nothing unless the CLI happens to be run from the right place.
    if let Some(dir) = file.parent() {
        vcad_eval::resolve_mesh_paths(&mut doc, dir);
    }
    Ok(doc)
}

fn load_vcad_document_raw(file: &PathBuf) -> Result<vcad_ir::Document> {
    use std::fs;

    if is_loon(file) {
        return eval_loon(file);
    }
    use vcad_app::materializer::materialize;
    use vcad_app::migrate::{detect_format, FileFormat};
    use vcad_crdt::CrdtDocument;

    let bytes = fs::read(file)?;
    match detect_format(&bytes) {
        FileFormat::V2Crdt => {
            let crdt = CrdtDocument::load(&bytes)
                .map_err(|e| anyhow::anyhow!("CRDT load failed: {}", e))?;
            Ok(materialize(&crdt).document)
        }
        FileFormat::V1Json => {
            let json = std::str::from_utf8(&bytes)
                .map_err(|e| anyhow::anyhow!("invalid utf-8 in v1 JSON: {}", e))?;
            Ok(vcad_ir::Document::from_json(json)?)
        }
        FileFormat::Unknown => {
            // Not CRDT and not JSON — try v0.2 VCode (token-efficient text
            // format) before giving up, so `vcad info` / `vcad export` can
            // read VCode documents directly.
            let text = std::str::from_utf8(&bytes)
                .map_err(|_| anyhow::anyhow!("unrecognized .vcad file format"))?;
            vcad_ir::vcode::from_vcode(text.trim())
                .map_err(|e| anyhow::anyhow!("unrecognized .vcad file format (VCode parse: {e})"))
        }
    }
}

/// Locate the parameter-table file: the `--file` argument, else `$VCAD_PARAMS`,
/// else `params.loon` in the working directory.
fn params_file(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p);
    }
    if let Ok(p) = std::env::var("VCAD_PARAMS") {
        return Ok(PathBuf::from(p));
    }
    let default = PathBuf::from("params.loon");
    if default.exists() {
        return Ok(default);
    }
    anyhow::bail!(
        "no parameter-table file — pass --file <path>, set VCAD_PARAMS, or put a \
         params.loon in the working directory"
    )
}

fn load_variant_set(file: Option<PathBuf>) -> Result<(PathBuf, vcad_loon::variants::VariantSet)> {
    let path = params_file(file)?;
    let source =
        std::fs::read_to_string(&path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let set = vcad_loon::variants::parse(&source)
        .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    Ok((path, set))
}

fn run_params(command: ParamsCommands) -> Result<()> {
    match command {
        ParamsCommands::Resolve {
            variant,
            file,
            table,
        } => {
            let (path, set) = load_variant_set(file)?;
            let resolved = set
                .resolve(&variant)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            if table {
                println!(
                    "{} ({}), effective scale {}",
                    resolved.name,
                    resolved.chain.join(" → "),
                    resolved.effective_scale
                );
                let w = resolved
                    .params
                    .iter()
                    .map(|p| p.name.len())
                    .max()
                    .unwrap_or(0);
                for p in &resolved.params {
                    let value = format!("{}{}", p.value, p.unit.as_deref().unwrap_or(""));
                    println!(
                        "  {:<w$}  {:>10}  {}",
                        p.name,
                        value,
                        p.source.explain(),
                        w = w
                    );
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&resolved)?);
            }
        }
        ParamsCommands::Diff {
            a,
            b,
            file,
            json,
            exit_code,
        } => {
            let (path, set) = load_variant_set(file)?;
            let diff = set
                .diff(&a, &b)
                .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
            if json {
                println!("{}", serde_json::to_string_pretty(&diff)?);
            } else {
                print!("{}", diff.render());
            }
            if exit_code && !diff.entries.is_empty() {
                std::process::exit(1);
            }
        }
        ParamsCommands::List { file } => {
            let (path, set) = load_variant_set(file)?;
            println!("{}", path.display());
            let mut tables: Vec<_> = set.tables.values().collect();
            tables.sort_by(|a, b| a.name.cmp(&b.name));
            for t in tables {
                println!("  table {} ({} parameters)", t.name, t.order.len());
            }
            let mut variants: Vec<_> = set.variants.values().collect();
            variants.sort_by(|a, b| a.name.cmp(&b.name));
            for v in variants {
                let scale = match v.scale {
                    Some(s) => format!(", scale {s}"),
                    None => String::new(),
                };
                println!(
                    "  variant {} (from {}{}, {} overlays)",
                    v.name,
                    v.parent,
                    scale,
                    v.overlays.len()
                );
            }
        }
    }
    Ok(())
}

fn show_info(file: &PathBuf) -> Result<()> {
    let doc = load_vcad_document(file)?;

    println!("vcad document: {}", file.display());
    println!("  Version: {}", doc.version);
    println!("  Nodes: {}", doc.nodes.len());
    println!("  Materials: {}", doc.materials.len());
    println!("  Scene entries: {}", doc.roots.len());

    if !doc.roots.is_empty() {
        println!("\nScene:");
        for (i, entry) in doc.roots.iter().enumerate() {
            let node = doc.nodes.get(&entry.root);
            let name = node
                .and_then(|n| n.name.as_ref())
                .map(|s| s.as_str())
                .unwrap_or("unnamed");
            println!("  {}: {} (material: {})", i + 1, name, entry.material);
        }
    }

    // Evaluate with timing and show mesh stats + breakdown.
    match crate::app::evaluate_document_timed(&doc, false) {
        Ok((meshes, timing)) => {
            let total_tris: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
            let total_verts: usize = meshes.iter().map(|m| m.vertices.len() / 3).sum();
            println!("\nMesh stats:");
            println!("  Total triangles: {}", total_tris);
            println!("  Total vertices: {}", total_verts);
            print_timing(&timing);
        }
        Err(e) => {
            println!("\nFailed to evaluate: {}", e);
        }
    }

    Ok(())
}

/// Print an `EvalTiming` breakdown that mirrors the JS `Engine.logTiming`
/// formatter ([packages/engine/src/index.ts:221]). Phase summary line
/// followed by a per-node line sorted by `eval_ms` desc, filtered to
/// `eval_ms > 1ms`.
fn print_timing(timing: &vcad_eval::EvalTiming) {
    println!("\nTiming:");
    let mut summary = vec![format!("total:{:.0}ms", timing.total_ms)];
    if let Some(p) = timing.parse_ms {
        summary.push(format!("parse:{:.0}ms", p));
    }
    summary.push(format!("tess:{:.0}ms", timing.tessellate_ms));
    if let Some(s) = timing.serialize_ms {
        summary.push(format!("ser:{:.0}ms", s));
    }
    if timing.clash_ms > 0.5 {
        summary.push(format!("clash:{:.0}ms", timing.clash_ms));
    }
    if timing.assembly_ms > 0.5 {
        summary.push(format!("asm:{:.0}ms", timing.assembly_ms));
    }
    println!("  [TIMING] {}", summary.join(" "));

    let mut nodes: Vec<(&String, &vcad_eval::NodeTiming)> = timing.nodes.iter().collect();
    nodes.sort_by(|a, b| {
        b.1.eval_ms
            .partial_cmp(&a.1.eval_ms)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let hot: Vec<String> = nodes
        .iter()
        .filter(|(_, n)| n.eval_ms > 1.0)
        .map(|(id, n)| {
            if n.mesh_ms > 0.5 {
                format!("{}#{}:{:.0}ms(mesh:{:.0})", n.op, id, n.eval_ms, n.mesh_ms)
            } else {
                format!("{}#{}:{:.0}ms", n.op, id, n.eval_ms)
            }
        })
        .collect();
    if !hot.is_empty() {
        println!("           {}", hot.join(" > "));
    }
}

/// The `--floating-base` family of flags, shared by `import-urdf` and
/// `simulate`.
struct FloatingBase {
    /// Synthesize a world link + 6-DOF joint when the URDF declares none.
    enabled: bool,
    /// Link to attach it to (default: the tree's root link).
    link: Option<String>,
    /// Initial base height in mm, written as the joint's `parentAnchor.z`.
    spawn_height_mm: Option<f64>,
}

fn import_urdf(
    input: &PathBuf,
    output: &PathBuf,
    floating: FloatingBase,
    relative_meshes: bool,
) -> Result<()> {
    use std::fs;
    use vcad_kernel_urdf::UrdfReadOptions;

    // Import the URDF file
    // Relative to the OUTPUT's directory: that is what the loader resolves
    // against, and it is usually not the URDF's directory.
    let rel_base = if relative_meshes {
        let dir = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        fs::create_dir_all(&dir)?;
        Some(dir)
    } else {
        None
    };
    let opts = UrdfReadOptions {
        urdf_dir: input.parent().map(|p| p.to_path_buf()),
        floating_base: floating.enabled,
        floating_base_link: floating.link,
        spawn_height_mm: floating.spawn_height_mm,
        mesh_paths_relative_to: rel_base,
        ..UrdfReadOptions::default()
    };
    let doc = vcad_kernel_urdf::read_urdf_with_options(input, &opts)?;
    // Report what the document actually says, not what was requested — the
    // flag is a no-op on a URDF that already declares a floating joint unless
    // a spawn height was given, and a message that claims otherwise is how a
    // robot ends up silently spawning at the world origin.
    match doc
        .joints
        .iter()
        .flat_map(|js| js.iter())
        .find(|j| matches!(j.kind, vcad_ir::JointKind::Free))
    {
        Some(j) => println!(
            "Floating base: 6-DOF root joint '{}' at z = {} mm",
            j.id, j.parent_anchor.z
        ),
        None if floating.enabled => {
            println!("Floating base: requested but NOT created — the robot is welded to the world")
        }
        None => {}
    }

    // Write the document
    let json = doc.to_json()?;
    fs::write(output, json)?;

    // Count parts and joints
    let num_parts = doc.part_defs.as_ref().map(|p| p.len()).unwrap_or(0);
    let num_joints = doc.joints.as_ref().map(|j| j.len()).unwrap_or(0);

    println!(
        "Imported URDF {} parts, {} joints from {} to {}",
        num_parts,
        num_joints,
        input.display(),
        output.display()
    );
    Ok(())
}

fn simulate_file(
    input: &PathBuf,
    steps: u32,
    dt: f64,
    log_every: u32,
    package_roots: &[PathBuf],
    floating: FloatingBase,
) -> Result<()> {
    use vcad_kernel_physics::PhysicsWorld;
    use vcad_kernel_urdf::UrdfReadOptions;

    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let doc = match ext.as_str() {
        "urdf" | "xml" => {
            let opts = UrdfReadOptions {
                package_roots: package_roots.to_vec(),
                urdf_dir: input.parent().map(|p| p.to_path_buf()),
                floating_base: floating.enabled,
                floating_base_link: floating.link,
                spawn_height_mm: floating.spawn_height_mm,
                // This path simulates in place from a URDF; absolute paths are
                // right here, nothing is being written out to commit.
                mesh_paths_relative_to: None,
            };
            vcad_kernel_urdf::read_urdf_with_options(input, &opts)?
        }
        // `load_vcad_document` evaluates `.loon` source, auto-detects CRDT vs
        // v1 JSON, and anchors mesh references to the document's directory.
        "vcad" | "json" | "loon" => load_vcad_document(input)?,
        other => anyhow::bail!(
            "simulate: unsupported input extension '{}' (expected .vcad, .loon, or .urdf)",
            other
        ),
    };

    let robot_name = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("robot");
    let num_parts = doc.part_defs.as_ref().map(|p| p.len()).unwrap_or(0);
    let num_joints_doc = doc.joints.as_ref().map(|j| j.len()).unwrap_or(0);

    let mut world = PhysicsWorld::from_document(&doc)?;
    let joint_ids = world.joint_ids();

    println!(
        "robot '{}': {} parts, {} joints (URDF) -> {} actuated DOF",
        robot_name,
        num_parts,
        num_joints_doc,
        joint_ids.len()
    );
    println!(
        "stepping {} times at dt={:.6}s ({:.2} Hz, total {:.3}s sim time)",
        steps,
        dt,
        1.0 / dt,
        steps as f64 * dt,
    );

    let dt_f32 = dt as f32;
    for s in 1..=steps {
        world.step(dt_f32);
        if log_every > 0 && (s % log_every == 0 || s == steps) {
            let states = world.get_joint_states();
            let max_speed = states
                .values()
                .map(|js| js.velocity.abs())
                .fold(0.0_f64, f64::max);
            let max_pos = states
                .values()
                .map(|js| js.position.abs())
                .fold(0.0_f64, f64::max);
            println!(
                "  step {:>5} / {}  t={:.3}s  |q|_max={:.3}  |qdot|_max={:.3}",
                s,
                steps,
                s as f64 * dt,
                max_pos,
                max_speed,
            );
        }
    }

    println!("done. final joint states:");
    let states = world.get_joint_states();
    for jid in &joint_ids {
        if let Some(js) = states.get(jid) {
            println!(
                "  {:<32}  q={:>8.3}  qdot={:>8.3}",
                jid, js.position, js.velocity
            );
        }
    }

    Ok(())
}

fn export_urdf(doc: &vcad_ir::Document, output: &PathBuf) -> Result<()> {
    vcad_kernel_urdf::write_urdf(doc, output)?;

    // Count parts and joints
    let num_parts = doc
        .part_defs
        .as_ref()
        .map(|p| p.len())
        .unwrap_or(doc.roots.len());
    let num_joints = doc.joints.as_ref().map(|j| j.len()).unwrap_or(0);

    println!(
        "Exported URDF with {} links, {} joints to {}",
        num_parts,
        num_joints,
        output.display()
    );
    Ok(())
}

/// Export from a document (for REPL use).
pub fn export_file_from_doc(doc: &vcad_ir::Document, output: &PathBuf) -> Result<()> {
    let meshes = crate::app::evaluate_document(doc)?;

    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_lowercase().as_str() {
        "stl" => {
            let mut combined_verts = Vec::new();
            let mut combined_idxs = Vec::new();
            for mesh in &meshes {
                let base_idx = (combined_verts.len() / 3) as u32;
                combined_verts.extend_from_slice(&mesh.vertices);
                for idx in &mesh.indices {
                    combined_idxs.push(idx + base_idx);
                }
            }
            let stl_bytes = export_stl_bytes(&combined_verts, &combined_idxs)?;
            std::fs::write(output, stl_bytes)?;
        }
        "step" | "stp" => {
            export_step(doc, output)?;
        }
        "urdf" => {
            export_urdf(doc, output)?;
        }
        _ => {
            anyhow::bail!("Unknown output format: {}", ext);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
/// `vcad check` — printability lint on an exported mesh.
///
/// Exits the process with 1 on a dirty verdict rather than returning an error,
/// so CI sees a lint failure (findings already printed) and not a crash.
#[allow(clippy::too_many_arguments)]
fn run_check(
    input: &std::path::Path,
    orientation: &str,
    nozzle: f64,
    max_bridge: f64,
    crack_threshold: f64,
    max_overhang: f64,
    strict_overhangs: bool,
    pitch: Option<f64>,
    section_step: f64,
    allow_bridge: &[String],
    json: bool,
) -> Result<()> {
    use vcad_printcheck::{check_file, render_text, Options, Orientation};

    let orientation = Orientation::parse(orientation).ok_or_else(|| {
        anyhow::anyhow!("unknown orientation `{orientation}` (expected one of z, -z, x, -x, y, -y)")
    })?;
    let mut allow_bridges = Vec::new();
    for spec in allow_bridge {
        let (a, b) = spec
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("--allow-bridge wants Z0:Z1, got `{spec}`"))?;
        let lo: f64 = a
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("--allow-bridge: `{a}` is not a number"))?;
        let hi: f64 = b
            .trim()
            .parse()
            .map_err(|_| anyhow::anyhow!("--allow-bridge: `{b}` is not a number"))?;
        if hi < lo {
            anyhow::bail!("--allow-bridge {spec}: the range runs backwards");
        }
        allow_bridges.push((lo, hi));
    }

    let opts = Options {
        orientation,
        nozzle,
        max_bridge,
        crack_threshold,
        max_overhang,
        strict_overhangs,
        pitch: pitch.unwrap_or(nozzle),
        section_step,
        allow_bridges,
        ..Default::default()
    };
    let report = check_file(input, &opts)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print!("{}", render_text(&report));
    }
    if !report.ok {
        std::process::exit(1);
    }
    Ok(())
}

fn slice_file(
    input: &PathBuf,
    output: &PathBuf,
    profile: &str,
    layer_height: Option<f64>,
    wall_count: Option<u32>,
    infill: Option<u32>,
    support: bool,
    print_temp: Option<u32>,
    bed_temp: Option<u32>,
    smart: bool,
    explain: bool,
) -> Result<()> {
    use vcad_slicer::{SliceResult, SliceSettings};
    use vcad_slicer_gcode::{GcodeSettings, PrinterProfile};

    let doc = load_vcad_document(input)?;
    let meshes = crate::app::evaluate_document(&doc)?;

    if meshes.is_empty() {
        anyhow::bail!("No geometry to slice");
    }

    // Combine all meshes
    let mut combined_verts = Vec::new();
    let mut combined_idxs = Vec::new();
    for mesh in &meshes {
        let base_idx = (combined_verts.len() / 3) as u32;
        combined_verts.extend_from_slice(&mesh.vertices);
        for idx in &mesh.indices {
            combined_idxs.push(idx + base_idx);
        }
    }

    let mesh = vcad_kernel_tessellate::TriangleMesh {
        vertices: combined_verts,
        indices: combined_idxs,
        normals: Vec::new(),
        face_kinds: Vec::new(),
        face_ids: Vec::new(),
    };

    // Resolve printer profile
    let printer_profile = match profile {
        "bambu_x1c" => PrinterProfile::bambu_x1c(),
        "bambu_p1s" => PrinterProfile::bambu_p1s(),
        "bambu_a1" => PrinterProfile::bambu_a1(),
        "bambu_a1_mini" => PrinterProfile::bambu_a1_mini(),
        "ender3" => PrinterProfile::ender3(),
        "prusa_mk4" => PrinterProfile::prusa_mk4(),
        "voron_24" => PrinterProfile::voron_24(),
        _ => PrinterProfile::generic(),
    };

    // Build settings — use smart defaults from BRep analysis if requested
    let settings = if smart {
        // Try to get BRep data for analysis
        let solid = try_build_solid(&doc);
        if let Some(solid) = solid {
            if let Some(brep) = solid.brep() {
                let volume = solid.volume();
                let surface_area = solid.surface_area();
                let analysis =
                    vcad_slicer::analyze::analyze_for_printing(brep, volume, surface_area);
                let params = vcad_slicer::smart_defaults::PrinterParams {
                    nozzle_diameter: printer_profile.nozzle_diameter,
                    bed_x: printer_profile.bed_x,
                    bed_y: printer_profile.bed_y,
                    bed_z: printer_profile.bed_z,
                };
                let smart_defaults =
                    vcad_slicer::smart_defaults::recommend_settings(&analysis, &params);

                if explain {
                    println!("\nBRep Analysis:");
                    for note in &analysis.notes {
                        println!("  - {}", note);
                    }
                    println!("\nSmart Defaults:");
                    for rec in &smart_defaults.recommendations {
                        println!("  {} = {} — {}", rec.setting, rec.value, rec.reason);
                    }
                    println!();
                }

                // Allow CLI overrides on top of smart defaults
                let mut s = smart_defaults.settings;
                if let Some(lh) = layer_height {
                    s.layer_height = lh;
                }
                if let Some(wc) = wall_count {
                    s.wall_count = wc;
                }
                if let Some(inf) = infill {
                    s.infill_density = inf as f64 / 100.0;
                }
                if support {
                    s.support_enabled = true;
                }
                s
            } else {
                if explain {
                    println!("  Note: No BRep data available, using manual defaults");
                }
                SliceSettings {
                    layer_height: layer_height.unwrap_or(0.2),
                    wall_count: wall_count.unwrap_or(3),
                    infill_density: infill.map(|i| i as f64 / 100.0).unwrap_or(0.15),
                    support_enabled: support,
                    ..Default::default()
                }
            }
        } else {
            if explain {
                println!("  Note: Could not build solid for analysis, using manual defaults");
            }
            SliceSettings {
                layer_height: layer_height.unwrap_or(0.2),
                wall_count: wall_count.unwrap_or(3),
                infill_density: infill.map(|i| i as f64 / 100.0).unwrap_or(0.15),
                support_enabled: support,
                ..Default::default()
            }
        }
    } else {
        SliceSettings {
            layer_height: layer_height.unwrap_or(0.2),
            wall_count: wall_count.unwrap_or(3),
            infill_density: infill.map(|i| i as f64 / 100.0).unwrap_or(0.15),
            support_enabled: support,
            ..Default::default()
        }
    };

    println!("Slicing with profile: {}", printer_profile.name);
    println!(
        "  Layer height: {:.2}mm, Walls: {}, Infill: {}%, Support: {}",
        settings.layer_height,
        settings.wall_count,
        (settings.infill_density * 100.0) as u32,
        if settings.support_enabled {
            "on"
        } else {
            "off"
        }
    );

    let result: SliceResult = vcad_slicer::slice(&mesh, &settings)?;
    println!(
        "  {} layers, {:.1}g filament, ~{}",
        result.stats.layer_count,
        result.stats.filament_grams,
        format_duration(result.stats.print_time_seconds),
    );

    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext.to_lowercase().as_str() {
        "gcode" => {
            let pt = print_temp.unwrap_or(printer_profile.default_print_temp);
            let bt = bed_temp.unwrap_or(printer_profile.default_bed_temp);
            let gcode_settings = GcodeSettings {
                printer: printer_profile,
                print_temp: pt,
                bed_temp: bt,
                ..Default::default()
            };
            let gcode = vcad_slicer_gcode::generate_gcode(&result, gcode_settings);
            std::fs::write(output, gcode)?;
            println!("Exported G-code to {}", output.display());
        }
        "3mf" => {
            use vcad_slicer_bambu::{PrintSettings, ThreeMfModel};

            let pt = print_temp.unwrap_or(printer_profile.default_print_temp);
            let bt = bed_temp.unwrap_or(printer_profile.default_bed_temp);

            let mut model = ThreeMfModel::new(
                input
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("model")
                    .to_string(),
                mesh.vertices,
                mesh.indices,
            );
            model.settings = PrintSettings {
                layer_height: settings.layer_height,
                first_layer_height: settings.first_layer_height,
                wall_count: settings.wall_count,
                infill_density: settings.infill_density,
                print_temp: pt,
                bed_temp: bt,
                ..Default::default()
            };
            let bytes = model.to_bytes()?;
            std::fs::write(output, bytes)?;
            println!("Exported 3MF to {}", output.display());
        }
        _ => {
            anyhow::bail!("Unknown output format '{}'. Use .gcode or .3mf", ext);
        }
    }

    Ok(())
}

/// Try to build a Solid from the first root of a document for BRep analysis.
fn try_build_solid(doc: &vcad_ir::Document) -> Option<vcad_kernel::Solid> {
    use vcad_kernel::Solid;

    if doc.roots.is_empty() {
        return None;
    }

    let root_id = doc.roots[0].root;
    let root_node = doc.nodes.get(&root_id)?;

    match &root_node.op {
        vcad_ir::CsgOp::Cube { size } => Some(Solid::cube(size.x, size.y, size.z)),
        vcad_ir::CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Some(Solid::cylinder(
            *radius,
            *height,
            if *segments == 0 { 32 } else { *segments },
        )),
        vcad_ir::CsgOp::Sphere { radius, segments } => Some(Solid::sphere(
            *radius,
            if *segments == 0 { 32 } else { *segments },
        )),
        vcad_ir::CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Some(Solid::cone(
            *radius_bottom,
            *radius_top,
            *height,
            if *segments == 0 { 32 } else { *segments },
        )),
        vcad_ir::CsgOp::Torus {
            major_radius,
            minor_radius,
            segments,
        } => Some(Solid::torus(
            *major_radius,
            *minor_radius,
            if *segments == 0 { 32 } else { *segments },
        )),
        vcad_ir::CsgOp::Wedge { size } => Some(Solid::wedge(size.x, size.y, size.z)),
        vcad_ir::CsgOp::Prism {
            sides,
            radius,
            height,
        } => Some(Solid::prism(*sides, *radius, *height)),
        _ => None,
    }
}

fn format_duration(seconds: f64) -> String {
    let hours = (seconds / 3600.0) as u32;
    let minutes = ((seconds % 3600.0) / 60.0) as u32;
    if hours > 0 {
        format!("{}h {}m", hours, minutes)
    } else {
        format!("{}m", minutes)
    }
}

fn create_new(file: &PathBuf, template: &str) -> Result<()> {
    use vcad_ir::{CsgOp, Document, Node, SceneEntry, Vec3};

    let mut doc = Document::new();

    match template {
        "empty" => {
            // Empty document - nothing to add
        }
        "cube" => {
            doc.nodes.insert(
                1,
                Node {
                    id: 1,
                    name: Some("Cube".to_string()),
                    op: CsgOp::Cube {
                        size: Vec3::new(20.0, 20.0, 20.0),
                    },
                },
            );
            doc.roots.push(SceneEntry {
                root: 1,
                material: "default".to_string(),
                visible: None,
            });
        }
        "assembly" => {
            // Create a simple two-part assembly
            doc.nodes.insert(
                1,
                Node {
                    id: 1,
                    name: Some("Base".to_string()),
                    op: CsgOp::Cube {
                        size: Vec3::new(40.0, 40.0, 10.0),
                    },
                },
            );
            doc.nodes.insert(
                2,
                Node {
                    id: 2,
                    name: Some("Pillar".to_string()),
                    op: CsgOp::Cylinder {
                        radius: 5.0,
                        height: 30.0,
                        segments: 32,
                    },
                },
            );
            doc.nodes.insert(
                3,
                Node {
                    id: 3,
                    name: Some("Pillar Translated".to_string()),
                    op: CsgOp::Translate {
                        child: 2,
                        offset: Vec3::new(0.0, 0.0, 10.0),
                    },
                },
            );
            doc.roots.push(SceneEntry {
                root: 1,
                material: "default".to_string(),
                visible: None,
            });
            doc.roots.push(SceneEntry {
                root: 3,
                material: "default".to_string(),
                visible: None,
            });
        }
        _ => {
            anyhow::bail!("Unknown template: {}. Use: empty, cube, assembly", template);
        }
    }

    let json = doc.to_json()?;
    std::fs::write(file, json)?;
    println!("Created {} with template '{}'", file.display(), template);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn render_to_image(
    input: &PathBuf,
    output: &std::path::Path,
    width: u32,
    height: u32,
    azimuth: f64,
    elevation: f64,
    distance: Option<f64>,
    background: &str,
    up_axis: crate::render::UpAxis,
) -> Result<()> {
    use crate::render::{Camera, GraphicsOutput, RenderBuffer};

    // Load and evaluate document (`.loon` source is evaluated on the way in)
    let doc = load_vcad_document(input)?;
    let meshes = crate::app::evaluate_document(&doc)?;

    if meshes.is_empty() {
        anyhow::bail!("No geometry to render");
    }

    // Build triangle list
    let mut triangles = Vec::new();
    let color = [180u8, 180, 190];

    for mesh in &meshes {
        for tri in mesh.indices.chunks(3) {
            if tri.len() < 3 {
                continue;
            }
            let i0 = tri[0] as usize * 3;
            let i1 = tri[1] as usize * 3;
            let i2 = tri[2] as usize * 3;

            if i0 + 2 >= mesh.vertices.len()
                || i1 + 2 >= mesh.vertices.len()
                || i2 + 2 >= mesh.vertices.len()
            {
                continue;
            }

            triangles.push(crate::render::Triangle {
                v0: [
                    mesh.vertices[i0],
                    mesh.vertices[i0 + 1],
                    mesh.vertices[i0 + 2],
                ],
                v1: [
                    mesh.vertices[i1],
                    mesh.vertices[i1 + 1],
                    mesh.vertices[i1 + 2],
                ],
                v2: [
                    mesh.vertices[i2],
                    mesh.vertices[i2 + 1],
                    mesh.vertices[i2 + 2],
                ],
                color,
                pick_id: 0,
            });
        }
    }

    // Calculate bounding box for auto-distance
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for tri in &triangles {
        for v in [&tri.v0, &tri.v1, &tri.v2] {
            for i in 0..3 {
                min[i] = min[i].min(v[i]);
                max[i] = max[i].max(v[i]);
            }
        }
    }

    let center = [
        (min[0] + max[0]) / 2.0,
        (min[1] + max[1]) / 2.0,
        (min[2] + max[2]) / 2.0,
    ];
    let size = [max[0] - min[0], max[1] - min[1], max[2] - min[2]];
    let max_dim = size[0].max(size[1]).max(size[2]);

    // Setup camera
    let mut camera = Camera::with_up_axis(up_axis);
    let target = crate::render::Vec3::new(center[0], center[1], center[2]);
    let dist = distance.map(|d| d as f32).unwrap_or(max_dim * 2.5);
    camera.set_orbit(azimuth as f32, elevation as f32, dist, target);

    // Create render buffer
    let mut buffer = RenderBuffer::new(width, height);

    // Parse background color
    let (bg_r, bg_g, bg_b) = if background == "transparent" {
        (0, 0, 0) // Will be transparent in PNG
    } else {
        parse_hex_color(background).unwrap_or((26, 26, 46))
    };
    buffer.clear(bg_r, bg_g, bg_b);

    // Render
    crate::render::render_scene(&mut buffer, &triangles, &camera);

    // Save to file
    let gfx = GraphicsOutput::new();
    gfx.save_png(&buffer, output)?;

    println!(
        "Rendered {}x{} image to {}",
        width,
        height,
        output.display()
    );
    Ok(())
}

fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r, g, b))
}

fn apply_boolean(
    file: &PathBuf,
    op: BooleanOp,
    part_a: &str,
    part_b: &str,
    output: Option<&PathBuf>,
    result_name: Option<String>,
) -> Result<()> {
    use vcad_ir::{CsgOp, Node, SceneEntry};

    // `.loon` is source, not a build artifact: read it, but never write the
    // mutated JSON back over it — require an explicit destination.
    if is_loon(file) && output.is_none() {
        anyhow::bail!(
            "{} is loon source; pass --output <file.vcad> to write the result",
            file.display()
        );
    }
    let mut doc = load_vcad_document(file)?;

    // Find part IDs (by ID or name)
    let id_a = find_part_id(&doc, part_a)?;
    let id_b = find_part_id(&doc, part_b)?;

    // Create boolean operation node
    let next_id = doc.nodes.keys().copied().max().unwrap_or(0) + 1;
    let op_node = match op {
        BooleanOp::Union => CsgOp::Union {
            left: id_a,
            right: id_b,
        },
        BooleanOp::Difference => CsgOp::Difference {
            left: id_a,
            right: id_b,
        },
        BooleanOp::Intersection => CsgOp::Intersection {
            left: id_a,
            right: id_b,
        },
    };

    let op_name = match op {
        BooleanOp::Union => "Union",
        BooleanOp::Difference => "Difference",
        BooleanOp::Intersection => "Intersection",
    };

    doc.nodes.insert(
        next_id,
        Node {
            id: next_id,
            name: result_name.or_else(|| Some(format!("{} Result", op_name))),
            op: op_node,
        },
    );

    // Remove operands from scene, add result
    doc.roots.retain(|e| e.root != id_a && e.root != id_b);
    doc.roots.push(SceneEntry {
        root: next_id,
        material: "default".to_string(),
        visible: None,
    });

    // Save
    let output_path = output.unwrap_or(file);
    let json = doc.to_json()?;
    std::fs::write(output_path, json)?;

    println!(
        "Applied {} of {} and {} -> node {}",
        op_name, part_a, part_b, next_id
    );
    println!("Saved to {}", output_path.display());
    Ok(())
}

fn apply_transform(
    file: &PathBuf,
    part: &str,
    translate: Option<String>,
    rotate: Option<String>,
    scale: Option<String>,
    output: Option<&PathBuf>,
) -> Result<()> {
    use vcad_ir::{CsgOp, Node};

    // `.loon` is source, not a build artifact: read it, but never write the
    // mutated JSON back over it — require an explicit destination.
    if is_loon(file) && output.is_none() {
        anyhow::bail!(
            "{} is loon source; pass --output <file.vcad> to write the result",
            file.display()
        );
    }
    let mut doc = load_vcad_document(file)?;

    let part_id = find_part_id(&doc, part)?;
    let mut current_id = part_id;
    let mut next_id = doc.nodes.keys().copied().max().unwrap_or(0) + 1;

    // Apply transforms in order: scale -> rotate -> translate
    if let Some(ref s) = scale {
        let factors = parse_vec3(s)?;
        doc.nodes.insert(
            next_id,
            Node {
                id: next_id,
                name: None,
                op: CsgOp::Scale {
                    child: current_id,
                    factor: factors,
                },
            },
        );
        current_id = next_id;
        next_id += 1;
    }

    if let Some(ref r) = rotate {
        let angles = parse_vec3(r)?;
        doc.nodes.insert(
            next_id,
            Node {
                id: next_id,
                name: None,
                op: CsgOp::Rotate {
                    child: current_id,
                    angles,
                },
            },
        );
        current_id = next_id;
        next_id += 1;
    }

    if let Some(ref t) = translate {
        let offset = parse_vec3(t)?;
        doc.nodes.insert(
            next_id,
            Node {
                id: next_id,
                name: None,
                op: CsgOp::Translate {
                    child: current_id,
                    offset,
                },
            },
        );
        current_id = next_id;
    }

    // Update scene root
    for entry in &mut doc.roots {
        if entry.root == part_id {
            entry.root = current_id;
        }
    }

    // Save
    let output_path = output.unwrap_or(file);
    let json = doc.to_json()?;
    std::fs::write(output_path, json)?;

    println!("Transformed part {} -> node {}", part, current_id);
    println!("Saved to {}", output_path.display());
    Ok(())
}

fn find_part_id(doc: &vcad_ir::Document, part: &str) -> Result<u64> {
    // Try parsing as ID first
    if let Ok(id) = part.parse::<u64>() {
        if doc.nodes.contains_key(&id) {
            return Ok(id);
        }
    }

    // Search by name
    for (id, node) in &doc.nodes {
        if let Some(ref name) = node.name {
            if name == part || name.to_lowercase() == part.to_lowercase() {
                return Ok(*id);
            }
        }
    }

    anyhow::bail!("Part '{}' not found (specify ID or name)", part)
}

fn parse_vec3(s: &str) -> Result<vcad_ir::Vec3> {
    let parts: Vec<&str> = s.split(',').collect();
    match parts.len() {
        1 => {
            let v: f64 = parts[0].trim().parse()?;
            Ok(vcad_ir::Vec3::new(v, v, v))
        }
        3 => {
            let x: f64 = parts[0].trim().parse()?;
            let y: f64 = parts[1].trim().parse()?;
            let z: f64 = parts[2].trim().parse()?;
            Ok(vcad_ir::Vec3::new(x, y, z))
        }
        _ => anyhow::bail!("Expected 'x,y,z' or single value, got '{}'", s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vcad_ir::{CsgOp, Document, Node, SceneEntry, Vec3};

    fn cube_doc() -> Document {
        let mut doc = Document::new();
        doc.nodes.insert(
            1,
            Node {
                id: 1,
                name: Some("Cube 1".to_string()),
                op: CsgOp::Cube {
                    size: Vec3::new(10.0, 10.0, 10.0),
                },
            },
        );
        doc.roots.push(SceneEntry {
            root: 1,
            material: "default".to_string(),
            visible: None,
        });
        doc
    }

    #[test]
    fn write_glb_produces_valid_glb() {
        let doc = cube_doc();
        let out = std::env::temp_dir().join("vcad_cli_test_export.glb");
        let count = write_glb(&doc, &out).expect("GLB export failed");
        assert_eq!(count, 1);
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(&out).ok();
        // GLB header: magic "glTF", version 2, total length == file length.
        assert!(bytes.len() > 12);
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(bytes[8..12].try_into().unwrap()) as usize,
            bytes.len()
        );
    }

    #[test]
    fn write_step_produces_ap214_file() {
        let doc = cube_doc();
        let out = std::env::temp_dir().join("vcad_cli_test_export.step");
        let count = write_step(&doc, &out).expect("STEP export failed");
        assert_eq!(count, 1);
        let text = std::fs::read_to_string(&out).unwrap();
        std::fs::remove_file(&out).ok();
        assert!(text.starts_with("ISO-10303-21"));
    }
}
