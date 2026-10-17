//! vcad CLI - Full-featured parametric CAD in the terminal
//!
//! Provides both an interactive TUI editor and headless commands for
//! creating and manipulating 3D CAD models.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::{Path, PathBuf};

mod app;
mod chat_session;
mod fabprep;
mod input;
mod keybinding_adapter;
mod kit;
mod log_capture;
#[cfg(feature = "print-server")]
mod print_server;
mod raycast;
mod render;
mod repl;
mod telemetry;
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

    /// Lay parts out on print plates and write a Bambu/Prusa 3MF kit
    ///
    /// The spec is JSON: a list of parts (mesh path, plate index, XY centre),
    /// optional bed size and per-plate names. Parts are dropped to the plate
    /// in Z; overlaps and off-bed placements are errors, not warnings.
    Kit {
        /// Path to the kit spec JSON
        spec: PathBuf,
        /// Output .3mf (defaults to the spec path with a .3mf extension)
        #[arg(short, long)]
        output: Option<PathBuf>,
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
        /// Output file (format determined by extension: .stl, .glb, .step, .stp, .urdf, .kicad_sch)
        output: PathBuf,
        /// Refuse to write a STEP that contains tessellated (triangle-soup)
        /// faces. Without this flag such a fallback is a stderr warning
        /// naming the node and the reason; with it, it is an error.
        #[arg(long)]
        strict_brep: bool,
    },

    /// Autoroute a board (a .loon with EcadBoard / EcadPlace) and write .kicad_pcb
    Route {
        /// Input .vcad or .loon file with a board
        input: PathBuf,
        /// Output .kicad_pcb
        output: PathBuf,
        /// Trace width (mm); default: the board's default rule
        #[arg(long)]
        width: Option<f64>,
        /// Continuous current a net carries, NET=AMPS (repeatable). Nets whose
        /// IPC-2221 width outgrows a trace are poured instead of traced.
        #[arg(long = "current", value_name = "NET=AMPS")]
        currents: Vec<String>,
        /// Allow blind and buried vias (default: through vias only - most fabs
        /// charge extra for the others on a 4-layer board)
        #[arg(long)]
        blind_vias: bool,
        /// Microvia size "DIAMETER,DRILL" (mm) for vias one dielectric deep from
        /// an outer layer - an HDI board's via-in-pad (implies --blind-vias)
        #[arg(long)]
        microvia: Option<String>,
        /// One greedy pass, no rip-up / negotiation / repair / second pass:
        /// minutes, for iterating on placement and stackup
        #[arg(long)]
        fast: bool,
        /// Router effort multiplier: scales negotiation and rip-up budgets
        #[arg(long, default_value = "1.0")]
        effort: f64,
        /// Route these nets first, before the rest (repeatable)
        #[arg(long = "priority", value_name = "NET")]
        priority: Vec<String>,
        /// Route only these nets (repeatable); the rest stay unrouted - e.g. to
        /// route a DDR bus alone and pin it before the full route
        #[arg(long = "only", value_name = "NET")]
        only: Vec<String>,
        /// Multi-layer BGA escape before routing, over these signal layers in
        /// order (comma list, e.g. "F.Cu,In3.Cu,In6.Cu,B.Cu"): each fine-pitch
        /// field's signal balls escape on the first layer with room, the rest
        /// drop by microvia to the next. Laid as fixed copper.
        #[arg(long)]
        escape: Option<String>,
        /// Print which nets would get a synthesized copper pour, on which layer,
        /// and why - then stop without routing
        #[arg(long)]
        explain_pours: bool,
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
    /// Run a named DFM ruleset over an STL (e.g. the hobby 3-axis mill rules)
    Dfm {
        /// Mesh to check (.stl, binary or ASCII)
        input: PathBuf,
        /// Ruleset / process name: hobby-3axis-mill, cnc_3axis, fdm, …
        #[arg(long, default_value = "hobby-3axis-mill")]
        ruleset: String,
        /// TOML rule pack overriding the bundled one (same schema as vcad-kernel-dfm's rules/*.toml)
        #[arg(long)]
        pack: Option<PathBuf>,
        /// Emit the full report as JSON instead of the summary
        #[arg(long)]
        json: bool,
    },
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
    let _otel = telemetry::init();
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
        Some(Commands::Export {
            input,
            output,
            strict_brep,
        }) => {
            export_file(&input, &output, strict_brep)?;
        }
        Some(Commands::Route {
            input,
            output,
            width,
            currents,
            explain_pours,
            blind_vias,
            microvia,
            fast,
            effort,
            priority,
            only,
            escape,
        }) => {
            route_file(
                &input,
                &output,
                width,
                &currents,
                explain_pours,
                blind_vias,
                microvia.as_deref(),
                fast,
                effort,
                priority,
                only,
                escape.as_deref(),
            )?;
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
        Some(Commands::Kit { spec, output }) => {
            kit::run(&kit::KitArgs { spec, output })?;
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
        Some(Commands::Dfm {
            input,
            ruleset,
            pack,
            json,
        }) => {
            run_dfm_ruleset(&input, &ruleset, pack.as_deref(), json)?;
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

fn export_file(input: &PathBuf, output: &PathBuf, strict_brep: bool) -> Result<()> {
    use std::fs;

    let doc = load_vcad_document(input)?;

    let ext = output.extension().and_then(|e| e.to_str()).unwrap_or("");
    if ext.eq_ignore_ascii_case("loon") {
        return export_loon(&doc, output);
    }
    // Board: footprints, nets, pours (unrouted). `vcad route` routes it.
    if ext.eq_ignore_ascii_case("kicad_pcb") {
        let pcb = doc
            .pcb
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{} has no board (EcadBoard)", input.display()))?;
        fs::write(output, vcad_ecad_symbols::write_kicad_pcb_for_project(pcb))?;
        write_board_project(output, pcb)?;
        println!(
            "Exported board ({} footprints, {} nets) to {}",
            pcb.footprints.len(),
            pcb.nets.len(),
            output.display()
        );
        return Ok(());
    }
    // Schematic: no mesh evaluation - a circuit-only loon file has no geometry.
    if ext.eq_ignore_ascii_case("kicad_sch") {
        let sheet = doc
            .schematic
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{} has no schematic", input.display()))?;
        fs::write(output, vcad_ecad_symbols::write_kicad_sch(sheet))?;
        let nets = sheet.nets.as_ref().map_or(0, |n| n.len());
        println!(
            "Exported schematic ({} parts, {} nets) to {}",
            sheet.components.len(),
            nets,
            output.display()
        );
        return Ok(());
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
            export_step(&doc, output, strict_brep)?;
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

/// Route every net of a document's board with vcad's autorouter and write KiCad.
/// Multi-layer escape for every dense pin field (see `--escape`): signal balls
/// of nets that have somewhere to go and no hand-laid via yet. Adds the escape
/// copper to `pcb` as Manual (never ripped) and returns (traces, vias) added.
fn plan_bga_escapes(
    pcb: &mut vcad_ir::ecad::Pcb,
    layers: &[vcad_ir::ecad::PcbLayer],
    via_d: f64,
    via_drill: f64,
    width: f64,
) -> (usize, usize) {
    use vcad_ecad_pcb::router::escape;
    use vcad_ir::ecad::{CopperSource, Trace, Via};
    let copper: Vec<vcad_ir::ecad::PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let idx = |l: vcad_ir::ecad::PcbLayer| copper.iter().position(|&c| c == l).unwrap_or(0);
    let planes: std::collections::BTreeSet<String> =
        pcb.zones.iter().map(|z| z.net.clone()).collect();
    let mut pads_of: std::collections::HashMap<String, usize> = Default::default();
    for f in &pcb.footprints {
        for p in &f.pads {
            if let Some(n) = &p.net {
                *pads_of.entry(n.clone()).or_default() += 1;
            }
        }
    }
    let (n0t, n0v) = (pcb.traces.len(), pcb.vias.len());
    for field in escape::detect_pin_fields(pcb) {
        let (lo, hi) = field.bbox();
        let inside = |p: vcad_ir::Vec2| {
            p.x >= lo.x - 1e-6 && p.x <= hi.x + 1e-6 && p.y >= lo.y - 1e-6 && p.y <= hi.y + 1e-6
        };
        let has_via = |p: vcad_ir::Vec2, n: &str| {
            pcb.vias.iter().any(|v| {
                v.net == n && (v.position.x - p.x).abs() < 1e-3 && (v.position.y - p.y).abs() < 1e-3
            })
        };
        let mut endpoints = Vec::new();
        for f in &pcb.footprints {
            for p in &f.pads {
                let Some(n) = &p.net else { continue };
                let at = vcad_ecad_pcb::geometry::pad_world_position(f, p);
                if n.is_empty()
                    || n.starts_with("~nc~")
                    || planes.contains(n)
                    || pads_of[n] < 2
                    || !inside(at)
                    || has_via(at, n)
                {
                    continue;
                }
                // where the net goes: the centroid of its pads outside this field
                let outside: Vec<vcad_ir::Vec2> = pcb
                    .footprints
                    .iter()
                    .flat_map(|g| g.pads.iter().map(move |q| (g, q)))
                    .filter(|(_, q)| q.net.as_deref() == Some(n.as_str()))
                    .map(|(g, q)| vcad_ecad_pcb::geometry::pad_world_position(g, q))
                    .filter(|q| !inside(*q))
                    .collect();
                if outside.is_empty() {
                    continue;
                }
                let k = outside.len() as f64;
                let target = vcad_ir::Vec2::new(
                    outside.iter().map(|q| q.x).sum::<f64>() / k,
                    outside.iter().map(|q| q.y).sum::<f64>() / k,
                );
                endpoints.push((n.clone(), at, target));
            }
        }
        if endpoints.is_empty() {
            continue;
        }
        let fixed: Vec<(vcad_ir::Vec2, f64, Vec<vcad_ir::ecad::PcbLayer>)> = pcb
            .vias
            .iter()
            .map(|v| {
                let (a, b) = (
                    idx(v.start_layer).min(idx(v.end_layer)),
                    idx(v.start_layer).max(idx(v.end_layer)),
                );
                (v.position, v.diameter / 2.0, copper[a..=b].to_vec())
            })
            .collect();
        let session = vcad_ecad_pcb::RouteSession::from_pcb(pcb);
        let plans = escape::plan_multilayer_escapes(
            &session,
            pcb,
            &field,
            &endpoints,
            layers,
            &fixed,
            via_d / 2.0,
            width,
        );
        let mut per_layer = vec![0usize; layers.len()];
        for ((net, at, _), plan) in endpoints.iter().zip(plans) {
            let Some((li, plan)) = plan else { continue };
            per_layer[li] += 1;
            if plan.layer != copper[0] {
                pcb.vias.push(Via {
                    position: *at,
                    diameter: via_d,
                    drill: via_drill,
                    start_layer: copper[0],
                    end_layer: plan.layer,
                    net: net.clone(),
                    source: Some(CopperSource::Manual),
                });
            }
            pcb.traces.extend(plan.segments.iter().map(|(a, b)| Trace {
                start: *a,
                end: *b,
                width,
                layer: plan.layer,
                net: net.clone(),
                source: Some(CopperSource::Manual),
            }));
        }
        progress(format!(
            "escape: field ({:.1},{:.1})-({:.1},{:.1}): {} signal balls, per layer {:?}, {} left",
            lo.x,
            lo.y,
            hi.x,
            hi.y,
            endpoints.len(),
            layers
                .iter()
                .zip(&per_layer)
                .map(|(l, n)| format!("{l:?} {n}"))
                .collect::<Vec<_>>(),
            endpoints.len() - per_layer.iter().sum::<usize>()
        ));
    }
    (pcb.traces.len() - n0t, pcb.vias.len() - n0v)
}

/// Route checkpoints: each finished phase saved next to the output
/// (`<out>.ckpt/<phase>.json`) under a hash of the routing input (board,
/// options, width). A rerun with the same input resumes after the last phase
/// it finds: iterating on the repairs reuses a multi-minute first pass.
/// `VCAD_ROUTE_REDO=first|second` recomputes that phase and the ones after it
/// (after a router change); `VCAD_ROUTE_CHECKPOINT=0` turns them off.
mod checkpoint {
    use std::path::{Path, PathBuf};

    const PHASES: [&str; 2] = ["first", "second"];

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Saved<T> {
        key: String,
        value: T,
    }

    fn on() -> bool {
        std::env::var("VCAD_ROUTE_CHECKPOINT").map_or(true, |v| v != "0")
    }

    fn path(out: &Path, phase: &str) -> PathBuf {
        let mut d = out.as_os_str().to_owned();
        d.push(".ckpt");
        PathBuf::from(d).join(format!("{phase}.json"))
    }

    /// FNV-1a: stable across runs and toolchains, unlike `DefaultHasher`.
    pub fn key(parts: &[&[u8]]) -> String {
        let mut h: u64 = 0xcbf29ce484222325;
        for p in parts {
            for b in p.iter().chain(b"\0") {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
        }
        format!("{h:016x}")
    }

    pub fn load<T: serde::de::DeserializeOwned>(out: &Path, phase: &str, key: &str) -> Option<T> {
        if !on() {
            return None;
        }
        if let Ok(redo) = std::env::var("VCAD_ROUTE_REDO") {
            let r = PHASES.iter().position(|p| *p == redo).unwrap_or(0);
            let me = PHASES.iter().position(|p| *p == phase).unwrap_or(0);
            if me >= r {
                return None;
            }
        }
        let s: Saved<T> = serde_json::from_slice(&std::fs::read(path(out, phase)).ok()?).ok()?;
        (s.key == key).then_some(s.value)
    }

    pub fn save<T: serde::Serialize>(out: &Path, phase: &str, key: &str, value: &T) {
        if !on() {
            return;
        }
        let p = path(out, phase);
        let r = std::fs::create_dir_all(p.parent().unwrap())
            .map_err(|e| e.to_string())
            .and_then(|_| {
                serde_json::to_vec(&Saved {
                    key: key.to_string(),
                    value,
                })
                .map_err(|e| e.to_string())
            })
            .and_then(|b| std::fs::write(&p, b).map_err(|e| e.to_string()));
        if let Err(e) = r {
            eprintln!("warning: checkpoint {}: {e}", p.display());
        }
    }
}

#[allow(clippy::too_many_arguments)] // one per CLI flag
fn route_file(
    input: &PathBuf,
    output: &PathBuf,
    width: Option<f64>,
    currents: &[String],
    explain_pours: bool,
    blind_vias: bool,
    microvia: Option<&str>,
    fast: bool,
    effort: f64,
    priority: Vec<String>,
    only: Vec<String>,
    escape: Option<&str>,
) -> Result<()> {
    use vcad_ir::ecad::{CopperSource, Trace, Via};
    // RUST_LOG=info shows the router's own account (legalization, pruning)
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn"))
        .try_init();
    let doc = load_vcad_document(input)?;
    let mut pcb = doc
        .pcb
        .clone()
        .ok_or_else(|| anyhow::anyhow!("{} has no board (EcadBoard)", input.display()))?;
    let w = width.unwrap_or(pcb.rules.default_rules.trace_width);
    let t0 = std::time::Instant::now();
    let microvia: Option<(f64, f64)> = match microvia {
        Some(m) => {
            let (d, dr) = m
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("--microvia {m}: expected DIAMETER,DRILL"))?;
            Some((d.trim().parse()?, dr.trim().parse()?))
        }
        None => None,
    };
    let mut opts = vcad_ecad_pcb::router::auto::RouteOptions {
        through_vias_only: !blind_vias && microvia.is_none(),
        microvia,
        fast,
        ..Default::default()
    };
    // Inner layers carrying a whole-board plane stay solid: no traces there.
    let board_area = {
        let o = &pcb.outline.vertices;
        let n = o.len();
        (0..n)
            .map(|i| o[i].x * o[(i + 1) % n].y - o[(i + 1) % n].x * o[i].y)
            .sum::<f64>()
            .abs()
            / 2.0
    };
    opts.no_route_layers = pcb
        .zones
        .iter()
        .filter(|z| {
            !matches!(
                z.layer,
                vcad_ir::ecad::PcbLayer::FCu | vcad_ir::ecad::PcbLayer::BCu
            )
        })
        .filter(|z| {
            let n = z.outline.len();
            let a = (0..n)
                .map(|i| {
                    z.outline[i].x * z.outline[(i + 1) % n].y
                        - z.outline[(i + 1) % n].x * z.outline[i].y
                })
                .sum::<f64>()
                .abs()
                / 2.0;
            board_area > 0.0 && a >= 0.8 * board_area
        })
        .map(|z| z.layer)
        .collect();
    if !opts.no_route_layers.is_empty() {
        println!("plane layers kept trace-free: {:?}", opts.no_route_layers);
    }
    opts.effort = effort;
    opts.prune_dangling_copper = std::env::var_os("VCAD_ROUTER_PRUNE").is_some();
    opts.priority_nets = priority;
    for c in currents {
        let (net, amps) = c
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("--current {c}: expected NET=AMPS"))?;
        let amps: f64 = amps
            .parse()
            .map_err(|_| anyhow::anyhow!("--current {c}: bad amps"))?;
        opts.pour_policy.net_current_a.insert(net.to_string(), amps);
    }
    // Declared currents are the design intent: don't also guess pours from
    // rail-looking net names.
    if !currents.is_empty() {
        opts.pour_policy.pour_power_rails = false;
    }
    if explain_pours {
        let c = vcad_ecad_pcb::decide_pours(&pcb, &opts.pour_policy, &[]);
        if c.is_empty() {
            println!("pours: none synthesized");
        }
        for c in &c {
            println!(
                "pour {} on {:?}: {} of {} pads on that layer - {:?}",
                c.net, c.layer, c.pads_on_layer, c.demand_pads, c.reason
            );
        }
        let zones = vcad_ecad_pcb::synthesize_pours(&pcb, &opts.pour_policy, &[]);
        for z in &zones {
            println!(
                "  outline: {} on {:?}, {} vertices",
                z.net,
                z.layer,
                z.outline.len()
            );
        }
        if zones.len() < c.len() {
            println!("  {} of {} candidates got an outline", zones.len(), c.len());
        }
        return Ok(());
    }
    // What the router sees differs from the board in two ways:
    // - copper with no net (a connector's unplated holes, a package's
    //   unnumbered thermal pads) gets a private net each, so the router keeps
    //   clearance from it instead of treating it as nothing;
    // - pour regions on the outer layers are reinforcement: the router traces
    //   those nets (a plane on a signal layer gets cut into islands by other
    //   nets' traces, stranding pads), and KiCad fills the pour around the
    //   traces afterwards.
    let mut route_pcb = pcb.clone();
    for f in &mut route_pcb.footprints {
        for (k, p) in f.pads.iter_mut().enumerate() {
            if p.net.is_none() && p.layers.iter().any(|l| l.is_copper()) {
                p.net = Some(format!("~nc~{}~{}", f.reference, k));
            }
        }
    }
    // Via-in-pad: a hand-laid via (a BGA's microvia fanout) inside a pad of
    // its own net makes that pad reachable on every layer the via spans, so the
    // router leaves the ball on the inner layer instead of drilling its own via
    // between balls.
    {
        let manual: Vec<&vcad_ir::ecad::Via> = route_pcb
            .vias
            .iter()
            .filter(|v| matches!(v.source, Some(CopperSource::Manual)))
            .collect::<Vec<_>>()
            .into_iter()
            .collect();
        let stack: Vec<vcad_ir::ecad::PcbLayer> = route_pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let mut grow: Vec<(usize, usize, Vec<vcad_ir::ecad::PcbLayer>)> = Vec::new();
        for (fi, f) in route_pcb.footprints.iter().enumerate() {
            for (pi, p) in f.pads.iter().enumerate() {
                let Some(net) = &p.net else { continue };
                let c = vcad_ecad_pcb::geometry::pad_world_position(f, p);
                for v in manual.iter().filter(|v| &v.net == net) {
                    if (v.position.x - c.x).hypot(v.position.y - c.y) < 0.05 {
                        let span: Vec<_> = stack
                            .iter()
                            .copied()
                            .filter(|l| l.spanned_by(v.start_layer, v.end_layer))
                            .collect();
                        grow.push((fi, pi, span));
                    }
                }
            }
        }
        for (fi, pi, span) in grow {
            let ls = &mut route_pcb.footprints[fi].pads[pi].layers;
            for l in span {
                if !ls.contains(&l) {
                    ls.push(l);
                }
            }
        }
    }
    // Unplated holes keep copper on every layer a hole clearance away; the
    // router sees each as a no-tracks, no-vias keepout.
    let copper_layers: Vec<_> = route_pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let hole_gap = pcb.rules.default_rules.clearance + 0.05;
    let mut npth = Vec::new();
    for f in &route_pcb.footprints {
        for p in f
            .pads
            .iter()
            .filter(|p| p.pad_type == vcad_ir::ecad::PadType::NPTH)
        {
            let Some(d) = &p.drill else { continue };
            let c = vcad_ecad_pcb::geometry::pad_world_position(f, p);
            let r = d.diameter.max(d.oval_height.unwrap_or(0.0)) / 2.0 + hole_gap;
            let outline = (0..24)
                .map(|k| {
                    let a = k as f64 * std::f64::consts::TAU / 24.0;
                    // circumscribe the circle so chords never cut inside it
                    let rr = r / (std::f64::consts::PI / 24.0).cos();
                    vcad_ir::Vec2::new(c.x + rr * a.cos(), c.y + rr * a.sin())
                })
                .collect();
            npth.push(vcad_ir::ecad::Keepout {
                outline,
                layers: copper_layers.clone(),
                no_tracks: true,
                no_vias: true,
                no_pour: false,
                no_components: false,
            });
        }
    }
    route_pcb.keepouts.extend(npth);
    for z in &route_pcb.zones {
        opts.pour_policy
            .net_current_a
            .entry(z.net.clone())
            .or_insert(0.0);
    }
    route_pcb.zones.retain(|z| {
        !matches!(
            z.layer,
            vcad_ir::ecad::PcbLayer::FCu | vcad_ir::ecad::PcbLayer::BCu
        )
    });
    progress(format!("routing {} nets", route_pcb.nets.len()));
    let _route_span = tracing::info_span!("route", nets = route_pcb.nets.len()).entered();
    if let Some(layers) = escape {
        let layers: Vec<vcad_ir::ecad::PcbLayer> = layers
            .split(',')
            .map(|l| serde_json::from_str(&format!("\"{}\"", l.trim().replace('.', ""))))
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| anyhow::anyhow!("--escape: bad layer list: {e}"))?;
        let (vd, vdr) = microvia.unwrap_or((
            route_pcb.rules.default_rules.via_diameter,
            route_pcb.rules.default_rules.via_drill,
        ));
        let (t, v) = plan_bga_escapes(&mut route_pcb, &layers, vd, vdr, w);
        progress(format!("escape: laid {t} escape traces, {v} microvias"));
        // the board copy written out carries them too
        pcb.traces.extend(
            route_pcb.traces[route_pcb.traces.len() - t..]
                .iter()
                .cloned(),
        );
        pcb.vias
            .extend(route_pcb.vias[route_pcb.vias.len() - v..].iter().cloned());
    }
    let ckpt_key = checkpoint::key(&[
        &serde_json::to_vec(&serde_json::to_value(&route_pcb).unwrap_or_default())
            .unwrap_or_default(),
        format!("{opts:?} {w} {only:?}").as_bytes(),
    ]);
    let r = match checkpoint::load::<vcad_ecad_pcb::router::auto::RouteAllResult>(
        output, "first", &ckpt_key,
    ) {
        Some(r) => {
            progress("first pass: from checkpoint");
            r
        }
        None => {
            let r = vcad_ecad_pcb::router::auto::route_all_with_opts(&route_pcb, w, &only, &opts);
            checkpoint::save(output, "first", &ckpt_key, &r);
            r
        }
    };
    progress(format!(
        "first pass done: {} traces, {} vias",
        r.traces.len(),
        r.vias.len()
    ));
    let (vd, vdrill) = (
        pcb.rules.default_rules.via_diameter,
        pcb.rules.default_rules.via_drill,
    );
    pcb.traces.extend(r.traces.iter().map(|t| Trace {
        start: t.start,
        end: t.end,
        width: t.width,
        layer: t.layer,
        net: t.net.clone(),
        source: Some(CopperSource::Autoroute),
    }));
    pcb.vias.extend(r.vias.iter().map(|v| Via {
        position: v.position,
        diameter: vd,
        drill: vdrill,
        start_layer: v.start_layer,
        end_layer: v.end_layer,
        net: v.net.clone(),
        source: Some(CopperSource::Autoroute),
    }));
    pcb.zones.extend(r.zones.iter().cloned());
    if let Some(raw) = std::env::var_os("VCAD_ROUTE_RAW") {
        std::fs::write(&raw, vcad_ecad_symbols::write_kicad_pcb_for_project(&pcb))?;
    }
    // Outer-layer pours are reinforcement the router ignored; KiCad's fill
    // carves overlapping ones and drops islands, so they can't be counted on
    // to connect anything. Judge the clean-up below without them.
    let outer_zones: Vec<_> = pcb
        .zones
        .iter()
        .filter(|z| {
            matches!(
                z.layer,
                vcad_ir::ecad::PcbLayer::FCu | vcad_ir::ecad::PcbLayer::BCu
            )
        })
        .cloned()
        .collect();
    pcb.zones.retain(|z| {
        !matches!(
            z.layer,
            vcad_ir::ecad::PcbLayer::FCu | vcad_ir::ecad::PcbLayer::BCu
        )
    });
    // the router's own leftovers: copper islands reaching no pad (spur
    // pruning is left out: it cut live links in multi-hop routes)
    let (dt, dv) = vcad_ecad_pcb::drc::prune_dangling_copper(&mut pcb);
    if dt + dv > 0 {
        println!("pruned dead copper: {dt} traces, {dv} vias");
    }
    let anchored = anchor_track_ends(&mut pcb);
    if anchored > 0 {
        println!("anchored {anchored} track ends into their pads");
    }
    let restored = restore_layer_change_vias(&route_pcb, &mut pcb);
    if restored > 0 {
        println!("restored {restored} layer-change vias");
    }
    // whatever is still split - unrouted, or opened by the clean-up above
    let all_nets: Vec<String> = pcb.nets.iter().map(|n| n.name.clone()).collect();
    let islands0 = vcad_ecad_pcb::drc::net_islands(&pcb, &all_nets);
    let mut split: Vec<String> = all_nets
        .iter()
        .filter(|n| islands0.get(*n).copied().unwrap_or(0) > 1)
        .cloned()
        .collect();
    if !only.is_empty() {
        split.retain(|n| only.contains(n));
    }
    split.sort();
    if !split.is_empty() {
        println!("split after clean-up: {}", split.join(", "));
    }
    // Second pass for split nets: rip a signal net's copper and route it
    // again against the finished board (plane nets keep theirs - the pass
    // only adds the missing joins).
    let second = checkpoint::load::<(vcad_ir::ecad::Pcb, Vec<String>)>(output, "second", &ckpt_key)
        .filter(|_| !fast);
    if let Some((p, sp)) = second {
        progress("second pass: from checkpoint");
        pcb = p;
        split = sp;
    } else {
        for _round in 0..(if fast { 0 } else { 3 }) {
            if split.is_empty() {
                break;
            }
            let planes: std::collections::BTreeSet<&str> =
                route_pcb.zones.iter().map(|z| z.net.as_str()).collect();
            let ripped: Vec<&String> = split
                .iter()
                .filter(|n| !planes.contains(n.as_str()))
                .collect();
            let mut again = route_pcb.clone();
            let keep = |n: &str| !ripped.iter().any(|r| r.as_str() == n);
            // copper laid by hand (EcadVia / EcadTrace: a BGA's microvia fanout) is
            // never ripped - the second pass routes to it like the first
            let manual = |s: &Option<CopperSource>| matches!(s, Some(CopperSource::Manual));
            again.traces = pcb
                .traces
                .iter()
                .filter(|t| keep(&t.net) || manual(&t.source))
                .cloned()
                .collect();
            again.vias = pcb
                .vias
                .iter()
                .filter(|v| keep(&v.net) || manual(&v.source))
                .cloned()
                .collect();
            progress(format!("second pass on {} split nets", split.len()));
            let _pass_span = tracing::info_span!("second_pass", split = split.len()).entered();
            // the second pass only joins what the first left split: one greedy
            // round, no negotiation / rip-up / joint repair - those re-run the
            // whole first pass's search budget on ~all nets (548 s on the brain)
            let mut opts2 = opts.clone();
            opts2.fast = true;
            let r2 = vcad_ecad_pcb::router::auto::route_all_with_opts(&again, w, &only, &opts2);
            let before: usize = vcad_ecad_pcb::drc::net_islands(&pcb, &split).values().sum();
            let mut trial = pcb.clone();
            trial.traces.retain(|t| keep(&t.net) || manual(&t.source));
            trial.vias.retain(|v| keep(&v.net) || manual(&v.source));
            let in_split = |n: &str| split.iter().any(|s| s == n);
            trial.traces.extend(
                r2.traces
                    .iter()
                    .filter(|t| in_split(&t.net))
                    .map(|t| Trace {
                        start: t.start,
                        end: t.end,
                        width: t.width,
                        layer: t.layer,
                        net: t.net.clone(),
                        source: Some(CopperSource::Autoroute),
                    }),
            );
            trial
                .vias
                .extend(r2.vias.iter().filter(|v| in_split(&v.net)).map(|v| Via {
                    position: v.position,
                    diameter: vd,
                    drill: vdrill,
                    start_layer: v.start_layer,
                    end_layer: v.end_layer,
                    net: v.net.clone(),
                    source: Some(CopperSource::Autoroute),
                }));
            anchor_track_ends(&mut trial);
            let after: usize = vcad_ecad_pcb::drc::net_islands(&trial, &split)
                .values()
                .sum();
            println!(
                "second pass on {}: islands {before} -> {after}",
                split.join(", ")
            );
            if after < before {
                pcb = trial;
                let isl = vcad_ecad_pcb::drc::net_islands(&pcb, &split);
                split.retain(|n| isl[n] > 1);
            } else {
                break;
            }
        }
        if !fast {
            checkpoint::save(output, "second", &ckpt_key, &(&pcb, &split));
        }
    }
    // --fast: the repair helpers below rebuild the session or the connectivity
    // per candidate, which on a dense board costs more than the route itself
    // VCAD_ROUTE_SKIP=links,stitch: leave a repair out (to measure what each adds)
    let skip = |what: &str| {
        std::env::var("VCAD_ROUTE_SKIP").is_ok_and(|v| v.split(',').any(|s| s == what))
    };
    let links = if fast || skip("links") {
        Vec::new()
    } else {
        progress(format!("direct links on {} split nets", split.len()));
        direct_links(&route_pcb, &pcb, &split, w)
    };
    if !links.is_empty() {
        println!("direct-linked {} leftover pad pairs", links.len());
    }
    pcb.traces.extend(links);
    let (st, sv) = if fast || skip("stitch") {
        (Vec::new(), Vec::new())
    } else {
        progress(format!("stitching {} split nets to planes", split.len()));
        stitch_to_planes(&route_pcb, &pcb, &split, w)
    };
    progress("repairs done");
    if !sv.is_empty() {
        println!("stitched {} pads to their planes", sv.len());
    }
    pcb.traces.extend(st);
    pcb.vias.extend(sv);
    dedupe_copper(&mut pcb);
    let (lt, lv) = trim_leaf_copper(&mut pcb);
    if lt + lv > 0 {
        println!("trimmed leaf copper: {lt} traces, {lv} vias");
    }
    pcb.zones.extend(outer_zones);
    // Router vias one dielectric deep from an outer layer are microvias: the
    // maze cleared them at the microvia size, so write them at it.
    if let Some((md, mdrill)) = microvia {
        let copper: Vec<vcad_ir::ecad::PcbLayer> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let idx = |l: vcad_ir::ecad::PcbLayer| copper.iter().position(|&c| c == l);
        for v in pcb.vias.iter_mut() {
            if let (Some(a), Some(b)) = (idx(v.start_layer), idx(v.end_layer)) {
                if vcad_ecad_pcb::router::maze::is_microvia_span(&copper, a, b) {
                    v.diameter = md;
                    v.drill = mdrill;
                }
            }
        }
    }
    std::fs::write(output, vcad_ecad_symbols::write_kicad_pcb_for_project(&pcb))?;
    write_board_project(output, &pcb)?;
    println!(
        "Routed {:.1}% of connections in {:.1} s: {} traces, {} vias, {} synthesized pours -> {}",
        r.routability * 100.0,
        t0.elapsed().as_secs_f64(),
        r.traces.len(),
        r.vias.len(),
        r.zones.len(),
        output.display()
    );
    if !r.unrouted_nets.is_empty() {
        println!(
            "unrouted nets ({}): {}",
            r.unrouted_nets.len(),
            r.unrouted_nets.join(", ")
        );
        for d in &r.diagnostics {
            println!(
                "  {} ({:.2},{:.2}) -> ({:.2},{:.2}): blocked by {}",
                d.net,
                d.from.x,
                d.from.y,
                d.to.x,
                d.to.y,
                if d.blocking_nets.is_empty() {
                    "-".to_string()
                } else {
                    d.blocking_nets.join(", ")
                }
            );
        }
    }
    Ok(())
}

/// Write `<board>.kicad_pro` next to a standalone `.kicad_pcb`, so KiCad (and
/// `kicad-cli pcb drc`) checks the board against its own design rules.
fn write_board_project(board: &std::path::Path, pcb: &vcad_ir::ecad::Pcb) -> Result<()> {
    if board.as_os_str() == "/dev/null" {
        return Ok(());
    }
    let name = board
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("board");
    std::fs::write(
        board.with_extension("kicad_pro"),
        vcad_ecad_symbols::write_kicad_pro_for_board(name, pcb),
    )?;
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

/// `vcad dfm <stl> --ruleset <name>`: run a named DFM ruleset over a mesh
/// and print pass/fail per rule with located examples and affordances.
/// Exit status is non-zero when any rule fails.
fn run_dfm_ruleset(
    input: &Path,
    ruleset: &str,
    pack_path: Option<&Path>,
    json: bool,
) -> Result<()> {
    use vcad_kernel::vcad_kernel_dfm::{run_dfm_mesh, DfmSeverity, RulePack};
    use vcad_printcheck::mesh::{Mesh, Orientation};

    let pack = match pack_path {
        Some(p) => RulePack::from_toml(&std::fs::read_to_string(p)?)
            .with_context(|| format!("parse rule pack {}", p.display()))?,
        None => RulePack::named(ruleset)
            .with_context(|| format!("unknown ruleset '{ruleset}' (try hobby-3axis-mill)"))?,
    };
    let mesh = Mesh::load_stl(input, Orientation::ZUp)?;
    let report = run_dfm_mesh(&mesh.tris, &pack);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} — {} ({} triangles)",
            report.rule_pack_name,
            input.display(),
            mesh.tris.len()
        );
        for v in &report.rule_results {
            println!(
                "  {} {:<28} {}",
                if v.passed { "PASS" } else { "FAIL" },
                v.label,
                v.summary
            );
            for a in &v.affordances {
                println!("       -> {a}");
            }
            for issue in report.issues.iter().filter(|i| i.rule == v.rule).take(6) {
                let tag = match issue.severity {
                    DfmSeverity::Error => "error",
                    DfmSeverity::Warning => "warn",
                    DfmSeverity::Info => "info",
                };
                println!("       [{tag}] {}", issue.message);
            }
        }
        if report.rule_results.is_empty() {
            println!("  (ruleset '{ruleset}' has no mesh-domain rules; nothing checked)");
        }
        let failed = report.rule_results.iter().filter(|v| !v.passed).count();
        println!(
            "  {} of {} rules pass, score {}",
            report.rule_results.len() - failed,
            report.rule_results.len(),
            report.score()
        );
    }
    if report.rule_results.iter().any(|v| !v.passed) {
        std::process::exit(1);
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

fn export_step(doc: &vcad_ir::Document, output: &PathBuf, strict_brep: bool) -> Result<()> {
    let count = write_step_strict(doc, output, strict_brep)?;
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
    write_step_strict(doc, output, false)
}

/// [`write_step`], with the option to refuse a tessellated result.
///
/// Every root whose solid is not a clean analytic B-rep is reported on
/// stderr, naming the document node and the reason the writer fell back to
/// facets. With `strict_brep` the same finding is an error instead: a CAM
/// deliverable that silently arrives as 200 000 triangles is worse than one
/// that fails to build.
fn write_step_strict(
    doc: &vcad_ir::Document,
    output: &PathBuf,
    strict_brep: bool,
) -> Result<usize> {
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

    // Loud fallback: name every root that will serialize as facets, the
    // node that lost the representation, and why. This used to be
    // completely silent — a 265 MB "STEP" of one-triangle planes looked
    // exactly like a successful export.
    let mut degraded_roots: Vec<String> = Vec::new();
    for (i, root) in roots.iter().enumerate() {
        let Some(solid) = root.solid.as_ref() else {
            continue;
        };
        let label = match &root.name {
            Some(name) => format!("'{name}' (node {})", root.node_id),
            None => format!("node {} (part_{})", root.node_id, i + 1),
        };
        if solid.fidelity() != vcad_kernel::SolidFidelity::Analytic {
            let why = solid
                .why_not_brep()
                .unwrap_or_else(|| "no analytic surfaces".to_string());
            eprintln!("warning: STEP export of {label} is tessellated, not B-rep: {why}");
            for event in solid.degradations() {
                eprintln!("  - {event}");
            }
            degraded_roots.push(label);
        } else {
            // Wrong-geometry events do not degrade the *representation*,
            // but they mean a cut was skipped: still worth saying.
            for event in solid.wrong_geometry_events() {
                eprintln!("warning: {label}: {event}");
            }
        }
    }
    if strict_brep && !degraded_roots.is_empty() {
        anyhow::bail!(
            "--strict-brep: {} of {} root(s) would be written as tessellated facets: {}",
            degraded_roots.len(),
            roots.len(),
            degraded_roots.join(", ")
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
            export_step(doc, output, false)?;
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

#[allow(clippy::too_many_arguments)]
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

/// Extend each trace end that touches a same-net pad without landing inside
/// it, so the end sits on the pad centre. The router counts copper overlap as
/// a connection; KiCad wants the track's end point inside the pad.
fn anchor_track_ends(pcb: &mut vcad_ir::ecad::Pcb) -> usize {
    use vcad_ir::ecad::PadShape;
    let mut pads = Vec::new();
    for f in &pcb.footprints {
        for p in &f.pads {
            let Some(net) = &p.net else { continue };
            let (w, h) = match &p.shape {
                PadShape::Circle { diameter } => (*diameter, *diameter),
                PadShape::Rect { width, height }
                | PadShape::Oval { width, height }
                | PadShape::RoundRect { width, height, .. } => (*width, *height),
                _ => continue,
            };
            let c = vcad_ecad_pcb::geometry::pad_world_position(f, p);
            let a = (f.rotation + p.rotation).to_radians();
            pads.push((net.clone(), p.layers.clone(), c, a, w / 2.0, h / 2.0));
        }
    }
    // (inside, touching) for point q against a pad, `grow` added for touch
    let test = |q: vcad_ir::Vec2, c: vcad_ir::Vec2, a: f64, hw: f64, hh: f64, grow: f64| {
        let (dx, dy) = (q.x - c.x, q.y - c.y);
        let (u, v) = (dx * a.cos() + dy * a.sin(), -dx * a.sin() + dy * a.cos());
        let inside = u.abs() <= hw - 1e-3 && v.abs() <= hh - 1e-3;
        let touch = u.abs() <= hw + grow && v.abs() <= hh + grow;
        (inside, touch)
    };
    let mut extra = Vec::new();
    for t in &pcb.traces {
        for q in [t.start, t.end] {
            let mut hits = pads
                .iter()
                .filter(|p| p.0 == t.net && p.1.contains(&t.layer))
                .map(|p| (test(q, p.2, p.3, p.4, p.5, t.width / 2.0), p.2));
            if hits.clone().any(|((inside, _), _)| inside) {
                continue;
            }
            if let Some((_, c)) = hits.find(|((_, touch), _)| *touch) {
                let mut seg = t.clone();
                seg.start = q;
                seg.end = c;
                extra.push(seg);
            }
        }
    }
    // a via that touches a same-net SMD pad it does not sit inside: the
    // router counts the overlap, KiCad needs copper on the pad's layer
    let tw = pcb.rules.default_rules.trace_width;
    for v in &pcb.vias {
        for p in pads.iter().filter(|p| p.0 == v.net) {
            let ((inside, touch), _) = (test(v.position, p.2, p.3, p.4, p.5, v.diameter / 2.0), ());
            if inside || !touch {
                continue;
            }
            let Some(&layer) = p.1.iter().find(|l| l.is_copper()) else {
                continue;
            };
            let joined = pcb.traces.iter().chain(extra.iter()).any(|t| {
                t.net == v.net
                    && t.layer == layer
                    && [t.start, t.end]
                        .iter()
                        .any(|e| (e.x - v.position.x).hypot(e.y - v.position.y) < 1e-3)
            });
            if joined || !p.1.iter().all(|l| *l == layer || !l.is_copper()) {
                continue;
            }
            extra.push(vcad_ir::ecad::Trace {
                start: v.position,
                end: p.2,
                width: tw,
                layer,
                net: v.net.clone(),
                source: Some(vcad_ir::ecad::CopperSource::Autoroute),
            });
        }
    }
    let n = extra.len();
    pcb.traces.extend(extra);
    n
}

/// Last resort for connections the router left open: join near pairs of
/// same-net pads with a straight or L-shaped trace on a layer they share,
/// accepted only where the router's own legality oracle says it is clear.
/// Route progress on stderr (unbuffered, unlike a redirected stdout), with
/// the time since the route started: `[  95.2s] stitch 12/40 VDD_CPU_LIT_S0`.
fn progress(msg: impl std::fmt::Display) {
    static T0: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let t = T0
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64();
    eprintln!("[{t:7.1}s] {msg}");
}

/// Wall-clock cap for each leftover repair (direct links, plane stitching):
/// they try candidate vias / stubs per pad and re-check the net's continuity
/// after each, which on a dense board ran for many hours.
const REPAIR_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

#[tracing::instrument(skip_all, fields(nets = nets.len()))]
fn direct_links(
    route_pcb: &vcad_ir::ecad::Pcb,
    pcb: &vcad_ir::ecad::Pcb,
    nets: &[String],
    width: f64,
) -> Vec<vcad_ir::ecad::Trace> {
    use vcad_ecad_pcb::spatial::CopperGeom;
    use vcad_ir::ecad::{CopperSource, Trace};
    let mut work = route_pcb.clone();
    work.traces = pcb.traces.clone();
    work.vias = pcb.vias.clone();
    let mut out = Vec::new();
    // plane nets join through their plane: stitch_to_planes does those, and
    // pairing a GND's ~800 pads here was O(pads^2) whole-board sessions
    let planes: std::collections::BTreeSet<String> =
        route_pcb.zones.iter().map(|z| z.net.clone()).collect();
    let fills = vcad_ecad_pcb::copper_pour::fill_zones(&work);
    vcad_ecad_pcb::drc::with_frozen_fills(fills, || {
        let t0 = std::time::Instant::now();
        for (ni, net) in nets.iter().enumerate() {
            if t0.elapsed() > REPAIR_BUDGET {
                progress(format!(
                    "link: {:?} budget spent, {} nets left as they are",
                    REPAIR_BUDGET,
                    nets.len() - ni
                ));
                break;
            }
            if planes.contains(net) {
                continue;
            }
            progress(format!("link {}/{} {net}", ni + 1, nets.len()));
            let _net_span = tracing::info_span!("link_net", net = %net).entered();
            let pads: Vec<_> = work
                .footprints
                .iter()
                .flat_map(|f| {
                    f.pads
                        .iter()
                        .filter(|p| p.net.as_deref() == Some(net.as_str()))
                        .map(move |p| {
                            (
                                vcad_ecad_pcb::geometry::pad_world_position(f, p),
                                p.layers.clone(),
                            )
                        })
                })
                .collect();
            let mut pads = pads;
            pads.extend(
                dangling_ends(&work)
                    .into_iter()
                    .filter(|e| &e.1 == net)
                    .map(|e| (e.0, vec![e.2])),
            );
            // one session and island count per net, rebuilt only after a link
            // is kept (a rejected link leaves the board as it was)
            let mut session = vcad_ecad_pcb::RouteSession::from_pcb(&work);
            let mut before = vcad_ecad_pcb::drc::analyze_net_continuity(&work, net).islands;
            'pairs: for i in 0..pads.len() {
                for j in i + 1..pads.len() {
                    if before <= 1 || t0.elapsed() > REPAIR_BUDGET {
                        break 'pairs;
                    }
                    let (a, b) = (pads[i].0, pads[j].0);
                    if (a.x - b.x).hypot(a.y - b.y) > 6.0 {
                        continue;
                    }
                    let Some(&layer) = pads[i]
                        .1
                        .iter()
                        .find(|l| l.is_copper() && pads[j].1.contains(l))
                    else {
                        continue;
                    };
                    let w = session.width_for(net, width);
                    let clearance = session.clearance_for(net);
                    let corners = [
                        vec![a, b],
                        vec![a, vcad_ir::Vec2::new(a.x, b.y), b],
                        vec![a, vcad_ir::Vec2::new(b.x, a.y), b],
                    ];
                    let legal = corners.into_iter().find(|path| {
                        path.windows(2).all(|s| {
                            let g = CopperGeom::Segment {
                                a: s[0],
                                b: s[1],
                                half_w: w / 2.0,
                            };
                            let pr = session.probe(&g, layer, net, clearance);
                            if !pr.legal && std::env::var_os("VCAD_LINK_DEBUG").is_some() {
                                let who: Vec<_> = pr
                                    .blockers
                                    .iter()
                                    .map(|b| {
                                        format!(
                                            "{}@{:.3}{:?}",
                                            b.net,
                                            b.distance,
                                            session.span_bounds(b.span)
                                        )
                                    })
                                    .collect();
                                eprintln!("link {net} {:?}->{:?}: {:?}", s[0], s[1], who);
                            }
                            pr.legal
                        })
                    });
                    let Some(path) = legal else { continue };
                    // keep the link only if it joins two islands of the net
                    let n0 = work.traces.len();
                    work.traces.extend(path.windows(2).map(|s| Trace {
                        start: s[0],
                        end: s[1],
                        width: w,
                        layer,
                        net: net.clone(),
                        source: Some(CopperSource::Autoroute),
                    }));
                    let after = vcad_ecad_pcb::drc::analyze_net_continuity(&work, net).islands;
                    if after < before {
                        out.extend(work.traces[n0..].iter().cloned());
                        before = after;
                        session = vcad_ecad_pcb::RouteSession::from_pcb(&work);
                    } else {
                        work.traces.truncate(n0);
                    }
                }
            }
        }
    });
    out
}

/// Pads of a plane net that the router left without a via to the plane: drop
/// one nearby with a short stub, where the legality oracle allows it, and keep
/// it only if it joins two islands of the net.
#[tracing::instrument(skip_all, fields(nets = nets.len()))]
fn stitch_to_planes(
    route_pcb: &vcad_ir::ecad::Pcb,
    pcb: &vcad_ir::ecad::Pcb,
    nets: &[String],
    width: f64,
) -> (Vec<vcad_ir::ecad::Trace>, Vec<vcad_ir::ecad::Via>) {
    use vcad_ecad_pcb::spatial::CopperGeom;
    use vcad_ir::ecad::{CopperSource, PcbLayer, Trace, Via};
    let mut work = route_pcb.clone();
    work.traces = pcb.traces.clone();
    work.vias = pcb.vias.clone();
    let (vd, vdrill) = (
        pcb.rules.default_rules.via_diameter,
        pcb.rules.default_rules.via_drill,
    );
    let copper: Vec<PcbLayer> = work
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let (front, back) = (copper[0], copper[copper.len() - 1]);
    let (mut ts, mut vs) = (Vec::new(), Vec::new());
    let fills = vcad_ecad_pcb::copper_pour::fill_zones(&work);
    vcad_ecad_pcb::drc::with_frozen_fills(fills, || {
        let t0 = std::time::Instant::now();
        for (ni, net) in nets.iter().enumerate() {
            if t0.elapsed() > REPAIR_BUDGET {
                progress(format!(
                    "stitch: {:?} budget spent, {} nets left as they are",
                    REPAIR_BUDGET,
                    nets.len() - ni
                ));
                break;
            }
            progress(format!("stitch {}/{} {net}", ni + 1, nets.len()));
            let _net_span = tracing::info_span!("stitch_net", net = %net).entered();
            if !route_pcb.zones.iter().any(|z| &z.net == net) {
                continue;
            }
            let pads: Vec<_> = work
                .footprints
                .iter()
                .flat_map(|f| {
                    f.pads
                        .iter()
                        .filter(|p| p.net.as_deref() == Some(net.as_str()) && p.drill.is_none())
                        .map(move |p| {
                            let (hw, hh) = match &p.shape {
                                vcad_ir::ecad::PadShape::Circle { diameter } => {
                                    (diameter / 2.0, diameter / 2.0)
                                }
                                vcad_ir::ecad::PadShape::Rect { width, height }
                                | vcad_ir::ecad::PadShape::Oval { width, height }
                                | vcad_ir::ecad::PadShape::RoundRect { width, height, .. } => {
                                    (width / 2.0, height / 2.0)
                                }
                                _ => (0.5, 0.5),
                            };
                            (
                                vcad_ecad_pcb::geometry::pad_world_position(f, p),
                                p.layers.clone(),
                                hw.max(hh),
                            )
                        })
                })
                .collect();
            // only pads off the net's main island need a stitch: trying all
            // ~800 GND pads x 169 via spots each was hours on the brain
            let stranded = vcad_ecad_pcb::drc::stranded_pads(&work, net);
            let pads: Vec<_> = pads
                .into_iter()
                .filter(|(c, _, _)| {
                    stranded
                        .iter()
                        .any(|s| (s.x - c.x).abs() < 1e-6 && (s.y - c.y).abs() < 1e-6)
                })
                .collect();
            progress(format!("  {net}: {} stranded pads", pads.len()));
            let ends: Vec<_> = dangling_ends(&work)
                .into_iter()
                .filter(|e| &e.1 == net)
                .map(|e| (e.0, vec![e.2], 0.0))
                .collect();
            let mut session = vcad_ecad_pcb::RouteSession::from_pcb(&work);
            let mut before = vcad_ecad_pcb::drc::analyze_net_continuity(&work, net).islands;
            for (c, layers, r) in ends.into_iter().chain(pads) {
                let Some(&layer) = layers.iter().find(|l| l.is_copper()) else {
                    continue;
                };
                if t0.elapsed() > REPAIR_BUDGET {
                    break;
                }
                if before <= 1 {
                    break;
                }
                let w = session.width_for(net, width);
                let clearance = session.clearance_for(net);
                'found: for ring in [-1.0, 0.5, 0.8, 1.1, 1.5, 2.0, 2.6] {
                    for k in 0..if ring < 0.0 { 1 } else { 24 } {
                        let a = k as f64 * std::f64::consts::TAU / 24.0;
                        // ring < 0: via in the pad itself
                        let d = if ring < 0.0 {
                            0.0
                        } else {
                            r + vd / 2.0 + ring - 0.4
                        };
                        let v = vcad_ir::Vec2::new(c.x + d * a.cos(), c.y + d * a.sin());
                        if !session.probe_via_drill(v, net).legal {
                            continue;
                        }
                        let disc = CopperGeom::Disc {
                            center: v,
                            r: vd / 2.0,
                        };
                        if !copper
                            .iter()
                            .all(|l| session.probe(&disc, *l, net, clearance).legal)
                        {
                            continue;
                        }
                        let stub = CopperGeom::Segment {
                            a: c,
                            b: v,
                            half_w: w / 2.0,
                        };
                        if !session.probe(&stub, layer, net, clearance).legal {
                            continue;
                        }
                        let n0 = (work.traces.len(), work.vias.len());
                        work.vias.push(Via {
                            position: v,
                            diameter: vd,
                            drill: vdrill,
                            start_layer: front,
                            end_layer: back,
                            net: net.clone(),
                            source: Some(CopperSource::Autoroute),
                        });
                        if d > 0.0 {
                            work.traces.push(Trace {
                                start: c,
                                end: v,
                                width: w,
                                layer,
                                net: net.clone(),
                                source: Some(CopperSource::Autoroute),
                            });
                        }
                        let after = vcad_ecad_pcb::drc::analyze_net_continuity(&work, net).islands;
                        if std::env::var_os("VCAD_LINK_DEBUG").is_some() {
                            eprintln!("stitch {net} pad ({:.2},{:.2}) via ({:.2},{:.2}): islands {before} -> {after}", c.x, c.y, v.x, v.y);
                        }
                        if after < before {
                            before = after;
                            session = vcad_ecad_pcb::RouteSession::from_pcb(&work);
                            vs.push(work.vias[n0.1].clone());
                            ts.extend(work.traces[n0.0..].iter().cloned());
                            break 'found;
                        }
                        work.traces.truncate(n0.0);
                        work.vias.truncate(n0.1);
                    }
                }
            }
        }
    });
    (ts, vs)
}

/// A trace end that meets same-net copper only on another layer (a pad on the
/// front, a trace on an inner layer) needs a via there; put back any the
/// router's output is missing.
fn restore_layer_change_vias(
    route_pcb: &vcad_ir::ecad::Pcb,
    pcb: &mut vcad_ir::ecad::Pcb,
) -> usize {
    use vcad_ir::ecad::{CopperSource, PcbLayer, Via};
    let near = |a: vcad_ir::Vec2, b: vcad_ir::Vec2| (a.x - b.x).hypot(a.y - b.y) < 1e-3;
    let copper: Vec<PcbLayer> = pcb
        .stackup
        .layers
        .iter()
        .map(|l| l.layer)
        .filter(|l| l.is_copper())
        .collect();
    let (front, back) = (copper[0], copper[copper.len() - 1]);
    // (point, net, layer) of every pad centre and trace end
    let mut ends: Vec<(vcad_ir::Vec2, String, PcbLayer)> = Vec::new();
    for f in &pcb.footprints {
        for p in &f.pads {
            let Some(net) = &p.net else { continue };
            let c = vcad_ecad_pcb::geometry::pad_world_position(f, p);
            for l in p.layers.iter().filter(|l| l.is_copper()) {
                ends.push((c, net.clone(), *l));
            }
        }
    }
    for t in &pcb.traces {
        ends.push((t.start, t.net.clone(), t.layer));
        ends.push((t.end, t.net.clone(), t.layer));
    }
    let mut work = route_pcb.clone();
    work.traces = pcb.traces.clone();
    work.vias = pcb.vias.clone();
    let session = vcad_ecad_pcb::RouteSession::from_pcb(&work);
    let vr = pcb.rules.default_rules.via_diameter / 2.0;
    let via_fits = |q: vcad_ir::Vec2, net: &str| {
        let disc = vcad_ecad_pcb::spatial::CopperGeom::Disc { center: q, r: vr };
        session.probe_via_drill(q, net).legal
            && copper.iter().all(|l| {
                session
                    .probe(&disc, *l, net, session.clearance_for(net))
                    .legal
            })
    };
    let mut added: Vec<Via> = Vec::new();
    for t in &pcb.traces {
        for q in [t.start, t.end] {
            let covered = pcb
                .vias
                .iter()
                .chain(added.iter())
                .any(|v| v.net == t.net && near(v.position, q));
            if covered {
                continue;
            }
            let other_layer = ends
                .iter()
                .any(|e| e.1 == t.net && e.2 != t.layer && near(e.0, q));
            if other_layer && via_fits(q, &t.net) {
                added.push(Via {
                    position: q,
                    diameter: pcb.rules.default_rules.via_diameter,
                    drill: pcb.rules.default_rules.via_drill,
                    start_layer: front,
                    end_layer: back,
                    net: t.net.clone(),
                    source: Some(CopperSource::Autoroute),
                });
            }
        }
    }
    let n = added.len();
    pcb.vias.extend(added);
    n
}

/// Trace ends that touch nothing of their net: no other trace end, via or pad.
fn dangling_ends(
    pcb: &vcad_ir::ecad::Pcb,
) -> Vec<(vcad_ir::Vec2, String, vcad_ir::ecad::PcbLayer)> {
    let near = |a: vcad_ir::Vec2, b: vcad_ir::Vec2, r: f64| (a.x - b.x).hypot(a.y - b.y) < r;
    // (net, layers, centre, angle, half-w, half-h)
    let mut pads = Vec::new();
    for f in &pcb.footprints {
        for p in &f.pads {
            let Some(net) = &p.net else { continue };
            let (w, h) = match &p.shape {
                vcad_ir::ecad::PadShape::Circle { diameter } => (*diameter, *diameter),
                vcad_ir::ecad::PadShape::Rect { width, height }
                | vcad_ir::ecad::PadShape::Oval { width, height }
                | vcad_ir::ecad::PadShape::RoundRect { width, height, .. } => (*width, *height),
                vcad_ir::ecad::PadShape::Custom { vertices } => {
                    let m = vertices
                        .iter()
                        .fold(0.0f64, |m, v| m.max(v.x.abs()).max(v.y.abs()));
                    (2.0 * m, 2.0 * m)
                }
            };
            pads.push((
                net.clone(),
                p.layers.clone(),
                vcad_ecad_pcb::geometry::pad_world_position(f, p),
                (f.rotation + p.rotation).to_radians(),
                w / 2.0,
                h / 2.0,
            ));
        }
    }
    let in_pad = |q: vcad_ir::Vec2, c: vcad_ir::Vec2, a: f64, hw: f64, hh: f64| {
        let (dx, dy) = (q.x - c.x, q.y - c.y);
        let (u, v) = (dx * a.cos() + dy * a.sin(), -dx * a.sin() + dy * a.cos());
        u.abs() <= hw + 1e-3 && v.abs() <= hh + 1e-3
    };
    let mut out = Vec::new();
    for (i, t) in pcb.traces.iter().enumerate() {
        for q in [t.start, t.end] {
            // another trace of the net whose copper the end sits on (end to
            // end, or a T onto its middle - KiCad joins both)
            let other_trace = pcb.traces.iter().enumerate().any(|(j, u)| {
                j != i
                    && u.net == t.net
                    && u.layer == t.layer
                    && seg_dist(q, u.start, u.end) <= u.width / 2.0 + 1e-3
            });
            let via = pcb
                .vias
                .iter()
                .any(|v| v.net == t.net && near(v.position, q, v.diameter / 2.0));
            let pad = pads
                .iter()
                .any(|p| p.0 == t.net && p.1.contains(&t.layer) && in_pad(q, p.2, p.3, p.4, p.5));
            if !other_trace && !via && !pad {
                out.push((q, t.net.clone(), t.layer));
            }
        }
    }
    out
}

/// Trim leaf copper KiCad would call dangling: traces with a free end and vias
/// that reach copper of their net on fewer than two layers, repeatedly. A
/// trace on a layer its net pours is never free (the pour picks it up). A net
/// whose continuity would get worse keeps its copper.
fn trim_leaf_copper(pcb: &mut vcad_ir::ecad::Pcb) -> (usize, usize) {
    let near = |a: vcad_ir::Vec2, b: vcad_ir::Vec2, r: f64| (a.x - b.x).hypot(a.y - b.y) < r;
    let poured: std::collections::BTreeSet<(String, vcad_ir::ecad::PcbLayer)> =
        pcb.zones.iter().map(|z| (z.net.clone(), z.layer)).collect();
    let orig = pcb.clone();
    let (mut nt, mut nv) = (0, 0);
    loop {
        let zones = pcb.zones.clone();
        let free: Vec<(vcad_ir::Vec2, String, vcad_ir::ecad::PcbLayer)> = dangling_ends(pcb)
            .into_iter()
            .filter(|e| {
                !zones
                    .iter()
                    .any(|z| z.net == e.1 && z.layer == e.2 && point_in(e.0, &z.outline))
            })
            .collect();
        let t0 = pcb.traces.len();
        pcb.traces.retain(|t| {
            !free.iter().any(|e| {
                e.1 == t.net
                    && e.2 == t.layer
                    && (near(e.0, t.start, 1e-3) || near(e.0, t.end, 1e-3))
            })
        });
        let dt = t0 - pcb.traces.len();
        // vias: layers on which the via meets its net's copper
        let pads: Vec<_> = pcb
            .footprints
            .iter()
            .flat_map(|f| {
                f.pads.iter().filter_map(move |p| {
                    Some((
                        vcad_ecad_pcb::geometry::pad_world_position(f, p),
                        p.net.clone()?,
                        p.layers.clone(),
                    ))
                })
            })
            .collect();
        let v0 = pcb.vias.len();
        let traces = pcb.traces.clone();
        let vias_all = pcb.vias.clone();
        let copper: Vec<vcad_ir::ecad::PcbLayer> = pcb
            .stackup
            .layers
            .iter()
            .map(|l| l.layer)
            .filter(|l| l.is_copper())
            .collect();
        let idx = |l: vcad_ir::ecad::PcbLayer| copper.iter().position(|&c| c == l);
        pcb.vias.retain(|v| {
            // hand-laid copper (a BGA's fanout) stays whether or not it was used
            if matches!(v.source, Some(vcad_ir::ecad::CopperSource::Manual)) {
                return true;
            }
            let mut layers: std::collections::BTreeSet<vcad_ir::ecad::PcbLayer> = traces
                .iter()
                .filter(|t| {
                    t.net == v.net
                        && (near(t.start, v.position, v.diameter / 2.0)
                            || near(t.end, v.position, v.diameter / 2.0))
                })
                .map(|t| t.layer)
                .collect();
            for p in pads
                .iter()
                .filter(|p| p.1 == v.net && near(p.0, v.position, 0.3))
            {
                layers.extend(p.2.iter().filter(|l| l.is_copper()).copied());
            }
            layers.extend(poured.iter().filter(|(n, _)| *n == v.net).map(|(_, l)| *l));
            // an HDI stack: another same-net via at this point whose span
            // meets this one's - their shared layer is a live end of it
            let span = |w: &vcad_ir::ecad::Via| {
                let (a, b) = (
                    idx(w.start_layer).unwrap_or(0),
                    idx(w.end_layer).unwrap_or(0),
                );
                (a.min(b), a.max(b))
            };
            let (v0_, v1_) = span(v);
            for w in vias_all.iter().filter(|w| {
                w.net == v.net
                    && near(w.position, v.position, v.diameter / 2.0)
                    && (w.start_layer, w.end_layer) != (v.start_layer, v.end_layer)
            }) {
                let (w0, w1) = span(w);
                let (lo, hi) = (w0.max(v0_), w1.min(v1_));
                if lo <= hi {
                    layers.extend(&copper[lo..=hi]);
                }
            }
            layers.len() >= 2
        });
        let dv = v0 - pcb.vias.len();
        nt += dt;
        nv += dv;
        if dt + dv == 0 {
            break;
        }
    }
    // never make a net worse
    let nets: std::collections::BTreeSet<String> =
        orig.traces.iter().map(|t| t.net.clone()).collect();
    let names: Vec<String> = nets.into_iter().collect();
    let (ia, ib) = (
        vcad_ecad_pcb::drc::net_islands(&orig, &names),
        vcad_ecad_pcb::drc::net_islands(pcb, &names),
    );
    for n in names {
        let (a, b) = (ia[&n], ib[&n]);
        if b > a {
            pcb.traces.retain(|t| t.net != n);
            pcb.vias.retain(|v| v.net != n);
            pcb.traces
                .extend(orig.traces.iter().filter(|t| t.net == n).cloned());
            pcb.vias
                .extend(orig.vias.iter().filter(|v| v.net == n).cloned());
        }
    }
    (nt, nv)
}

/// Even-odd point-in-polygon test.
fn point_in(p: vcad_ir::Vec2, poly: &[vcad_ir::Vec2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    inside
}

/// Distance from `p` to segment `a`-`b`.
fn seg_dist(p: vcad_ir::Vec2, a: vcad_ir::Vec2, b: vcad_ir::Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (a.x + t * dx - p.x).hypot(a.y + t * dy - p.y)
}

/// Drop exact duplicate traces and vias (clean-up passes can re-add a stub).
fn dedupe_copper(pcb: &mut vcad_ir::ecad::Pcb) {
    let key = |p: vcad_ir::Vec2| ((p.x * 1e4).round() as i64, (p.y * 1e4).round() as i64);
    let mut seen = std::collections::BTreeSet::new();
    pcb.traces.retain(|t| {
        let (a, b) = (key(t.start), key(t.end));
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        seen.insert((
            t.net.clone(),
            format!("{:?}", t.layer),
            a,
            b,
            (t.width * 1e4).round() as i64,
        ))
    });
    let mut seen = std::collections::BTreeSet::new();
    // a via's span is part of its identity: an HDI stack is two or more vias
    // at one point (F-In1 + In1-In3), and keying on position alone kept the
    // first and dropped the rest - the stack's lower hop vanished
    pcb.vias.retain(|v| {
        seen.insert((
            v.net.clone(),
            key(v.position),
            format!("{:?}{:?}", v.start_layer, v.end_layer),
        ))
    });
}
