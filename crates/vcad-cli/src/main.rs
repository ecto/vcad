//! vcad CLI - Full-featured parametric CAD in the terminal
//!
//! Provides both an interactive TUI editor and headless commands for
//! creating and manipulating 3D CAD models.

use anyhow::Result;
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

mod app;
mod chat_session;
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

    /// Export a .vcad file to another format
    Export {
        /// Input .vcad file
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
    },

    /// Render document to image
    Render {
        /// Input vcad file
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
    },

    /// Apply boolean operation
    Boolean {
        /// Input vcad file
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
        /// Input vcad file
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

    /// Display information about a .vcad file
    Info {
        /// Path to the .vcad file
        file: PathBuf,
    },

    /// Run a physics simulation on a robot assembly (.vcad or .urdf)
    Simulate {
        /// Input file (.vcad or .urdf). URDFs are imported in-memory.
        input: PathBuf,
        /// Number of simulation steps to run
        #[arg(long, default_value = "240")]
        steps: u32,
        /// Simulation timestep in seconds
        #[arg(long, default_value = "0.004166666")]
        dt: f64,
        /// Print joint states every N steps (0 = only summary)
        #[arg(long, default_value = "60")]
        log_every: u32,
        /// Search root for `package://NAME/...` URI resolution. Repeat for
        /// multiple roots. Each root is expected to contain `NAME/`
        /// subdirectories (the standard ROS package layout).
        #[arg(long = "package-root", value_name = "DIR")]
        package_roots: Vec<PathBuf>,
    },

    /// Slice a .vcad file for 3D printing
    Slice {
        /// Input .vcad file
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

    /// Start the print relay server for web app → printer communication
    #[cfg(feature = "print-server")]
    PrintServer {
        /// Port to listen on
        #[arg(long, default_value = "7878")]
        port: u16,
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

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum BooleanOp {
    Union,
    Difference,
    Intersection,
}

fn main() -> Result<()> {
    vcad_i18n::init(&vcad_i18n::Locale::from_env());
    let cli = Cli::parse();

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
        Some(Commands::ImportUrdf { input, output }) => {
            import_urdf(&input, &output)?;
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
        Some(Commands::Info { file }) => {
            show_info(&file)?;
        }
        Some(Commands::Simulate {
            input,
            steps,
            dt,
            log_every,
            package_roots,
        }) => {
            simulate_file(&input, steps, dt, log_every, &package_roots)?;
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
        #[cfg(feature = "print-server")]
        Some(Commands::PrintServer { port }) => {
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(print_server::start_server(port))?;
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

fn export_file(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use std::fs;

    let json = fs::read_to_string(input)?;
    let doc = vcad_ir::Document::from_json(&json)?;

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
            let stl_bytes = export_stl_bytes(&combined_verts, &combined_idxs)?;
            fs::write(output, stl_bytes)?;
            println!("Exported STL to {}", output.display());
        }
        "glb" => {
            println!("GLB export not yet implemented in CLI");
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

fn export_step(doc: &vcad_ir::Document, output: &PathBuf) -> Result<()> {
    use vcad_kernel::Solid;

    // For now, we can only export primitives that haven't been through booleans
    // We need to evaluate the document and check if B-rep is available

    // Simple case: single root with a primitive
    if doc.roots.is_empty() {
        anyhow::bail!("Document has no geometry to export");
    }

    // Get the first root and try to create geometry from it
    let root_id = doc.roots[0].root;
    let root_node = doc
        .nodes
        .get(&root_id)
        .ok_or_else(|| anyhow::anyhow!("Root node not found"))?;

    // Try to create a solid from the IR
    let solid = match &root_node.op {
        vcad_ir::CsgOp::Cube { size } => Solid::cube(size.x, size.y, size.z),
        vcad_ir::CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Solid::cylinder(
            *radius,
            *height,
            if *segments == 0 { 32 } else { *segments },
        ),
        vcad_ir::CsgOp::Sphere { radius, segments } => {
            Solid::sphere(*radius, if *segments == 0 { 32 } else { *segments })
        }
        vcad_ir::CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Solid::cone(
            *radius_bottom,
            *radius_top,
            *height,
            if *segments == 0 { 32 } else { *segments },
        ),
        vcad_ir::CsgOp::StepImport { path } => {
            // Re-read from the original STEP file
            Solid::from_step(path)?
        }
        _ => {
            anyhow::bail!(
                "STEP export only supports primitive shapes (cube, cylinder, sphere, cone) \
                 or previously imported STEP files. Boolean operations convert geometry to mesh \
                 which cannot be exported to STEP format."
            );
        }
    };

    solid.to_step(output)?;
    println!("Exported STEP to {}", output.display());
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
    let solids = Solid::from_step_all(input)?;

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
/// Auto-detects CRDT (v0.4) vs legacy v1 JSON shapes.
fn load_vcad_document(file: &PathBuf) -> Result<vcad_ir::Document> {
    use std::fs;
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
            anyhow::bail!("unrecognized .vcad file format")
        }
    }
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

fn import_urdf(input: &PathBuf, output: &PathBuf) -> Result<()> {
    use std::fs;

    // Import the URDF file
    let doc = vcad_kernel_urdf::read_urdf(input)?;

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
            };
            vcad_kernel_urdf::read_urdf_with_options(input, &opts)?
        }
        "vcad" | "json" => {
            let json = std::fs::read_to_string(input)?;
            vcad_ir::Document::from_json(&json)?
        }
        other => anyhow::bail!(
            "simulate: unsupported input extension '{}' (expected .vcad or .urdf)",
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

    let json = std::fs::read_to_string(input)?;
    let doc = vcad_ir::Document::from_json(&json)?;
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
) -> Result<()> {
    use crate::render::{Camera, GraphicsOutput, RenderBuffer};

    // Load and evaluate document
    let json = std::fs::read_to_string(input)?;
    let doc = vcad_ir::Document::from_json(&json)?;
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
    let mut camera = Camera::default();
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

    let json = std::fs::read_to_string(file)?;
    let mut doc = vcad_ir::Document::from_json(&json)?;

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

    let json = std::fs::read_to_string(file)?;
    let mut doc = vcad_ir::Document::from_json(&json)?;

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
