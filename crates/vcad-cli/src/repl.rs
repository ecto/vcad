//! Interactive REPL (Read-Eval-Print Loop) for vcad.
//!
//! Provides a command-line interface for creating and manipulating CAD geometry
//! without the full TUI.

use anyhow::Result;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use std::path::PathBuf;
use vcad_ir::{CsgOp, Document, Node, SceneEntry, Vec3};

/// REPL state
pub struct Repl {
    doc: Document,
    file_path: Option<PathBuf>,
    next_id: u64,
    modified: bool,
}

impl Repl {
    /// Create a new REPL, optionally loading from a file.
    pub fn new(file: Option<PathBuf>) -> Result<Self> {
        let doc = if let Some(ref path) = file {
            let json = std::fs::read_to_string(path)?;
            Document::from_json(&json)?
        } else {
            Document::new()
        };

        let next_id = doc.nodes.keys().copied().max().unwrap_or(0) + 1;

        Ok(Self {
            doc,
            file_path: file,
            next_id,
            modified: false,
        })
    }

    /// Run the REPL loop.
    pub fn run(&mut self) -> Result<()> {
        let mut rl = DefaultEditor::new()?;

        println!("vcad REPL - type 'help' for commands");
        if let Some(ref path) = self.file_path {
            println!("Loaded: {}", path.display());
        }

        loop {
            let prompt = if self.modified { "vcad*> " } else { "vcad> " };

            match rl.readline(prompt) {
                Ok(line) => {
                    let line = line.trim();
                    if line.is_empty() {
                        continue;
                    }

                    let _ = rl.add_history_entry(line);

                    if let Err(e) = self.execute(line) {
                        eprintln!("Error: {}", e);
                    }
                }
                Err(ReadlineError::Interrupted) => {
                    println!("Use 'quit' to exit");
                }
                Err(ReadlineError::Eof) => {
                    break;
                }
                Err(e) => {
                    eprintln!("Error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    fn execute(&mut self, line: &str) -> Result<()> {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0].to_lowercase().as_str() {
            // === Primitives ===
            "cube" | "box" => {
                let x = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let y = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(x);
                let z = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(y);
                let id = self.add_node(
                    CsgOp::Cube {
                        size: Vec3::new(x, y, z),
                    },
                    Some(format!("Cube {}", self.next_id)),
                );
                println!("Created cube {} ({} x {} x {})", id, x, y, z);
            }
            "cylinder" | "cyl" => {
                let r = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
                let h = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let id = self.add_node(
                    CsgOp::Cylinder {
                        radius: r,
                        height: h,
                        segments: 32,
                    },
                    Some(format!("Cylinder {}", self.next_id)),
                );
                println!("Created cylinder {} (r={}, h={})", id, r, h);
            }
            "sphere" => {
                let r = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
                let id = self.add_node(
                    CsgOp::Sphere {
                        radius: r,
                        segments: 32,
                    },
                    Some(format!("Sphere {}", self.next_id)),
                );
                println!("Created sphere {} (r={})", id, r);
            }
            "cone" => {
                let r1 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(5.0);
                let r2 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let h = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let id = self.add_node(
                    CsgOp::Cone {
                        radius_bottom: r1,
                        radius_top: r2,
                        height: h,
                        segments: 32,
                    },
                    Some(format!("Cone {}", self.next_id)),
                );
                println!("Created cone {} (r1={}, r2={}, h={})", id, r1, r2, h);
            }

            // === Booleans ===
            "union" => {
                let a: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: union <id1> <id2>"))?;
                let b: u64 = parts
                    .get(2)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: union <id1> <id2>"))?;
                let id = self.add_node(CsgOp::Union { left: a, right: b }, None);
                // Remove operands from scene roots (keep only result)
                self.doc.roots.retain(|e| e.root != a && e.root != b);
                println!("Created union {} of {} and {}", id, a, b);
            }
            "difference" | "diff" => {
                let a: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: difference <id1> <id2>"))?;
                let b: u64 = parts
                    .get(2)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: difference <id1> <id2>"))?;
                let id = self.add_node(CsgOp::Difference { left: a, right: b }, None);
                self.doc.roots.retain(|e| e.root != a && e.root != b);
                println!("Created difference {} of {} minus {}", id, a, b);
            }
            "intersection" | "inter" => {
                let a: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: intersection <id1> <id2>"))?;
                let b: u64 = parts
                    .get(2)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: intersection <id1> <id2>"))?;
                let id = self.add_node(CsgOp::Intersection { left: a, right: b }, None);
                self.doc.roots.retain(|e| e.root != a && e.root != b);
                println!("Created intersection {} of {} and {}", id, a, b);
            }

            // === Transforms ===
            "translate" | "move" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: translate <id> <x> <y> <z>"))?;
                let x = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let y = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let z = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let id = self.add_node(
                    CsgOp::Translate {
                        child: target,
                        offset: Vec3::new(x, y, z),
                    },
                    None,
                );
                // Replace target in scene roots with translated version
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!(
                    "Created translate {} of {} by ({}, {}, {})",
                    id, target, x, y, z
                );
            }
            "rotate" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: rotate <id> <rx> <ry> <rz>"))?;
                let rx = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let ry = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let rz = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let id = self.add_node(
                    CsgOp::Rotate {
                        child: target,
                        angles: Vec3::new(rx, ry, rz),
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!(
                    "Created rotate {} of {} by ({}, {}, {}) degrees",
                    id, target, rx, ry, rz
                );
            }
            "scale" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: scale <id> <sx> [sy] [sz]"))?;
                let sx = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let sy = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(sx);
                let sz = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(sy);
                let id = self.add_node(
                    CsgOp::Scale {
                        child: target,
                        factor: Vec3::new(sx, sy, sz),
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!(
                    "Created scale {} of {} by ({}, {}, {})",
                    id, target, sx, sy, sz
                );
            }

            // === Modifiers ===
            "fillet" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: fillet <id> <radius>"))?;
                let r = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let id = self.add_node(
                    CsgOp::Fillet {
                        child: target,
                        radius: r,
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!("Created fillet {} on {} with radius {}", id, target, r);
            }
            "chamfer" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: chamfer <id> <distance>"))?;
                let d = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let id = self.add_node(
                    CsgOp::Chamfer {
                        child: target,
                        distance: d,
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!("Created chamfer {} on {} with distance {}", id, target, d);
            }
            "shell" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: shell <id> <thickness>"))?;
                let t = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let id = self.add_node(
                    CsgOp::Shell {
                        child: target,
                        thickness: t,
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!("Created shell {} on {} with thickness {}", id, target, t);
            }

            // === Patterns ===
            "linear" | "lpattern" => {
                let target: u64 = parts.get(1).and_then(|s| s.parse().ok()).ok_or_else(|| {
                    anyhow::anyhow!("Usage: linear <id> <dx> <dy> <dz> <count> <spacing>")
                })?;
                let dx = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                let dy = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let dz = parts.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let count: u32 = parts.get(5).and_then(|s| s.parse().ok()).unwrap_or(3);
                let spacing = parts.get(6).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let id = self.add_node(
                    CsgOp::LinearPattern {
                        child: target,
                        direction: Vec3::new(dx, dy, dz),
                        count,
                        spacing,
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!("Created linear pattern {} with {} copies", id, count);
            }
            "circular" | "cpattern" => {
                let target: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: circular <id> <count> [angle]"))?;
                let count: u32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(6);
                let angle = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(360.0);
                let id = self.add_node(
                    CsgOp::CircularPattern {
                        child: target,
                        axis_origin: Vec3::new(0.0, 0.0, 0.0),
                        axis_dir: Vec3::new(0.0, 0.0, 1.0),
                        count,
                        angle_deg: angle,
                    },
                    None,
                );
                for entry in &mut self.doc.roots {
                    if entry.root == target {
                        entry.root = id;
                    }
                }
                println!("Created circular pattern {} with {} copies", id, count);
            }

            // === Document ===
            "list" | "ls" => {
                println!("Nodes:");
                let mut node_ids: Vec<_> = self.doc.nodes.keys().copied().collect();
                node_ids.sort();
                for id in node_ids {
                    if let Some(node) = self.doc.nodes.get(&id) {
                        let name = node.name.as_deref().unwrap_or("unnamed");
                        let op_name = op_type_name(&node.op);
                        let in_scene = self.doc.roots.iter().any(|e| e.root == id);
                        let marker = if in_scene { "*" } else { " " };
                        println!("  {}{}: {} ({})", marker, id, name, op_name);
                    }
                }
                println!("\n* = in scene ({} roots)", self.doc.roots.len());
            }
            "show" => {
                let id: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: show <id>"))?;
                if !self.doc.roots.iter().any(|e| e.root == id) {
                    self.doc.roots.push(SceneEntry {
                        root: id,
                        material: "default".into(),
                        visible: None,
                    });
                    self.modified = true;
                    println!("Added {} to scene", id);
                } else {
                    println!("Node {} is already in scene", id);
                }
            }
            "hide" => {
                let id: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: hide <id>"))?;
                let before = self.doc.roots.len();
                self.doc.roots.retain(|e| e.root != id);
                if self.doc.roots.len() < before {
                    self.modified = true;
                    println!("Removed {} from scene", id);
                } else {
                    println!("Node {} was not in scene", id);
                }
            }
            "delete" | "rm" => {
                let id: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: delete <id>"))?;
                self.doc.nodes.remove(&id);
                self.doc.roots.retain(|e| e.root != id);
                self.modified = true;
                println!("Deleted node {}", id);
            }
            "name" => {
                let id: u64 = parts
                    .get(1)
                    .and_then(|s| s.parse().ok())
                    .ok_or_else(|| anyhow::anyhow!("Usage: name <id> <name>"))?;
                let name = parts.get(2..).map(|p| p.join(" ")).unwrap_or_default();
                if let Some(node) = self.doc.nodes.get_mut(&id) {
                    node.name = Some(name.clone());
                    self.modified = true;
                    println!("Renamed node {} to '{}'", id, name);
                } else {
                    println!("Node {} not found", id);
                }
            }

            // === File operations ===
            "save" => {
                let path = parts
                    .get(1)
                    .map(PathBuf::from)
                    .or_else(|| self.file_path.clone());
                if let Some(path) = path {
                    let json = self.doc.to_json()?;
                    std::fs::write(&path, json)?;
                    self.file_path = Some(path.clone());
                    self.modified = false;
                    println!("Saved to {}", path.display());
                } else {
                    return Err(anyhow::anyhow!("No file path. Usage: save <path>"));
                }
            }
            "load" | "open" => {
                let path = parts
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: load <path>"))?;
                let json = std::fs::read_to_string(path)?;
                self.doc = Document::from_json(&json)?;
                self.file_path = Some(PathBuf::from(path));
                self.next_id = self.doc.nodes.keys().copied().max().unwrap_or(0) + 1;
                self.modified = false;
                println!("Loaded {}", path);
            }
            "new" => {
                self.doc = Document::new();
                self.file_path = None;
                self.next_id = 1;
                self.modified = false;
                println!("Created new document");
            }
            "export" => {
                let path = parts
                    .get(1)
                    .ok_or_else(|| anyhow::anyhow!("Usage: export <path.stl|.step>"))?;
                let path = PathBuf::from(path);
                crate::export_file_from_doc(&self.doc, &path)?;
                println!("Exported to {}", path.display());
            }
            "info" => {
                println!("Document info:");
                println!("  Nodes: {}", self.doc.nodes.len());
                println!("  Scene roots: {}", self.doc.roots.len());
                println!("  Materials: {}", self.doc.materials.len());
                if let Some(ref path) = self.file_path {
                    println!("  File: {}", path.display());
                }
                println!("  Modified: {}", self.modified);

                // Try to evaluate and show mesh stats
                match crate::app::evaluate_document(&self.doc) {
                    Ok(meshes) => {
                        let total_tris: usize = meshes.iter().map(|m| m.indices.len() / 3).sum();
                        let total_verts: usize = meshes.iter().map(|m| m.vertices.len() / 3).sum();
                        println!("  Triangles: {}", total_tris);
                        println!("  Vertices: {}", total_verts);
                    }
                    Err(e) => {
                        println!("  Evaluation error: {}", e);
                    }
                }
            }

            // === Help ===
            "help" | "?" => {
                print_help();
            }

            // === Exit ===
            "quit" | "exit" | "q" => {
                if self.modified {
                    println!("Unsaved changes. Use 'save' first or 'quit!' to discard.");
                } else {
                    std::process::exit(0);
                }
            }
            "quit!" => {
                std::process::exit(0);
            }

            _ => {
                println!(
                    "Unknown command: {}. Type 'help' for available commands.",
                    parts[0]
                );
            }
        }

        Ok(())
    }

    fn add_node(&mut self, op: CsgOp, name: Option<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;

        self.doc.nodes.insert(id, Node { id, name, op });

        // Auto-add to scene (only for primitives)
        self.doc.roots.push(SceneEntry {
            root: id,
            material: "default".into(),
            visible: None,
        });

        self.modified = true;
        id
    }
}

fn op_type_name(op: &CsgOp) -> &'static str {
    match op {
        CsgOp::Empty => "empty",
        CsgOp::Cube { .. } => "cube",
        CsgOp::Cylinder { .. } => "cylinder",
        CsgOp::Sphere { .. } => "sphere",
        CsgOp::Cone { .. } => "cone",
        CsgOp::Torus { .. } => "torus",
        CsgOp::Wedge { .. } => "wedge",
        CsgOp::Prism { .. } => "prism",
        CsgOp::Union { .. } => "union",
        CsgOp::Difference { .. } => "difference",
        CsgOp::Intersection { .. } => "intersection",
        CsgOp::Translate { .. } => "translate",
        CsgOp::Rotate { .. } => "rotate",
        CsgOp::Scale { .. } => "scale",
        CsgOp::Mirror { .. } => "mirror",
        CsgOp::Fillet { .. } => "fillet",
        CsgOp::Chamfer { .. } => "chamfer",
        CsgOp::EdgeBlendLoft { .. } => "edge blend loft",
        CsgOp::Shell { .. } => "shell",
        CsgOp::LinearPattern { .. } => "linear pattern",
        CsgOp::CircularPattern { .. } => "circular pattern",
        CsgOp::Sketch2D { .. } => "sketch",
        CsgOp::Extrude { .. } => "extrude",
        CsgOp::Revolve { .. } => "revolve",
        CsgOp::Text2D { .. } => "text",
        CsgOp::Sweep { .. } => "sweep",
        CsgOp::Loft { .. } => "loft",
        CsgOp::ImportedMesh { .. } => "imported mesh",
        CsgOp::StepImport { .. } => "step import",
        CsgOp::MeshImport { .. } => "mesh import",
        CsgOp::PcbBoard { .. } => "pcb board",
        CsgOp::EmbroideryPattern { .. } => "embroidery pattern",
        CsgOp::PartInstance { .. } => "part instance",
        CsgOp::SheetMetalBaseFlangeRect { .. } => "sheet-metal base flange",
        CsgOp::SheetMetalEdgeFlange { .. } => "sheet-metal edge flange",
        CsgOp::SheetMetalHem { .. } => "sheet-metal hem",
        CsgOp::SheetMetalJog { .. } => "sheet-metal jog",
        CsgOp::SheetMetalBaseFlangePolygon { .. } => "sheet-metal base flange (polygon)",
        CsgOp::SheetMetalBendRelief { .. } => "sheet-metal bend relief",
    }
}

fn print_help() {
    println!(
        r#"vcad REPL Commands:

Primitives:
  cube [x] [y] [z]              Create cube (default 10x10x10)
  cylinder [radius] [height]    Create cylinder (default r=5, h=10)
  sphere [radius]               Create sphere (default r=5)
  cone [r1] [r2] [height]       Create cone (default r1=5, r2=0, h=10)

Booleans:
  union <id1> <id2>             Union of two solids
  difference <id1> <id2>        Subtract id2 from id1
  intersection <id1> <id2>      Intersection of two solids

Transforms:
  translate <id> <x> <y> <z>    Translate solid
  rotate <id> <rx> <ry> <rz>    Rotate solid (degrees)
  scale <id> <sx> [sy] [sz]     Scale solid

Modifiers:
  fillet <id> <radius>          Fillet edges
  chamfer <id> <distance>       Chamfer edges
  shell <id> <thickness>        Hollow out solid

Patterns:
  linear <id> <dx> <dy> <dz> <count> <spacing>  Linear pattern
  circular <id> <count> [angle]                  Circular pattern

Document:
  list (ls)                     List all nodes
  show <id>                     Add node to scene
  hide <id>                     Remove node from scene
  delete <id>                   Delete node
  name <id> <name>              Rename node

File:
  new                           Create new document
  load <path>                   Load document
  save [path]                   Save document
  export <path>                 Export to STL/STEP
  info                          Show document info

Other:
  help                          Show this help
  quit                          Exit (warns if unsaved)
  quit!                         Exit without saving
"#
    );
}

/// Run the REPL with an optional initial file.
pub fn run_repl(file: Option<PathBuf>) -> Result<()> {
    let mut repl = Repl::new(file)?;
    repl.run()
}
