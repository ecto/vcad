//! TUI application state and main loop.

use anyhow::Result;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::{
    collections::HashSet,
    io::{self, Stdout},
    path::PathBuf,
    time::Duration,
};
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

use crate::render::{Camera, RenderBuffer, Triangle};
use crate::tui::{TuiMode, SketchPlane};
use crate::ui;

/// Mesh data from evaluation.
pub struct EvaluatedMesh {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

/// Application state.
pub struct App {
    /// The IR document being edited.
    pub document: Document,
    /// Currently selected node IDs.
    pub selected: HashSet<NodeId>,
    /// Camera for 3D viewport.
    pub camera: Camera,
    /// Whether the app is running.
    pub running: bool,
    /// Current TUI mode.
    pub mode: TuiMode,
    /// Command input buffer.
    pub command_input: String,
    /// Status message.
    pub status: String,
    /// Cached evaluated meshes.
    pub meshes: Vec<EvaluatedMesh>,
    /// Undo stack.
    undo_stack: Vec<Document>,
    /// Redo stack.
    redo_stack: Vec<Document>,
    /// Next node ID.
    next_node_id: NodeId,
    /// File path if opened from file.
    pub file_path: Option<PathBuf>,
}

impl App {
    /// Create a new application with optional initial document.
    pub fn new(file_path: Option<PathBuf>) -> Result<Self> {
        let document = if let Some(ref path) = file_path {
            let json = std::fs::read_to_string(path)?;
            Document::from_json(&json)?
        } else {
            Document::new()
        };

        let next_node_id = document.nodes.keys().copied().max().unwrap_or(0) + 1;

        let mut app = Self {
            document,
            selected: HashSet::new(),
            camera: Camera::default(),
            running: true,
            mode: TuiMode::Normal,
            command_input: String::new(),
            status: "Ready".to_string(),
            meshes: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_node_id,
            file_path,
        };

        // Initial evaluation
        app.evaluate()?;

        Ok(app)
    }

    /// Allocate a new node ID.
    fn alloc_node_id(&mut self) -> NodeId {
        let id = self.next_node_id;
        self.next_node_id += 1;
        id
    }

    /// Push current state to undo stack.
    fn push_undo(&mut self) {
        self.undo_stack.push(self.document.clone());
        self.redo_stack.clear();
        // Limit undo stack size
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> Result<()> {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.document.clone());
            self.document = prev;
            self.evaluate()?;
            self.status = "Undo".to_string();
        }
        Ok(())
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) -> Result<()> {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.document.clone());
            self.document = next;
            self.evaluate()?;
            self.status = "Redo".to_string();
        }
        Ok(())
    }

    /// Add a cube primitive.
    pub fn add_cube(&mut self, size: f64) -> Result<NodeId> {
        self.push_undo();
        let id = self.alloc_node_id();
        self.document.nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("Cube {}", id)),
                op: CsgOp::Cube {
                    size: Vec3::new(size, size, size),
                },
            },
        );
        self.document.roots.push(SceneEntry {
            root: id,
            material: "default".to_string(),
            visible: None,
        });
        self.evaluate()?;
        self.status = format!("Added cube {}", id);
        Ok(id)
    }

    /// Add a cylinder primitive.
    pub fn add_cylinder(&mut self, radius: f64, height: f64) -> Result<NodeId> {
        self.push_undo();
        let id = self.alloc_node_id();
        self.document.nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("Cylinder {}", id)),
                op: CsgOp::Cylinder {
                    radius,
                    height,
                    segments: 32,
                },
            },
        );
        self.document.roots.push(SceneEntry {
            root: id,
            material: "default".to_string(),
            visible: None,
        });
        self.evaluate()?;
        self.status = format!("Added cylinder {}", id);
        Ok(id)
    }

    /// Add a sphere primitive.
    pub fn add_sphere(&mut self, radius: f64) -> Result<NodeId> {
        self.push_undo();
        let id = self.alloc_node_id();
        self.document.nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("Sphere {}", id)),
                op: CsgOp::Sphere {
                    radius,
                    segments: 32,
                },
            },
        );
        self.document.roots.push(SceneEntry {
            root: id,
            material: "default".to_string(),
            visible: None,
        });
        self.evaluate()?;
        self.status = format!("Added sphere {}", id);
        Ok(id)
    }

    /// Delete selected nodes.
    pub fn delete_selected(&mut self) -> Result<()> {
        if self.selected.is_empty() {
            return Ok(());
        }
        self.push_undo();

        // Remove from roots
        self.document
            .roots
            .retain(|e| !self.selected.contains(&e.root));

        // Remove nodes (only if they're not referenced by remaining roots)
        // For simplicity, just remove the root nodes for now
        for &id in &self.selected {
            self.document.nodes.remove(&id);
        }

        let count = self.selected.len();
        self.selected.clear();
        self.evaluate()?;
        self.status = format!("Deleted {} part(s)", count);
        Ok(())
    }

    /// Translate selected nodes.
    pub fn translate_selected(&mut self, dx: f64, dy: f64, dz: f64) -> Result<()> {
        if self.selected.is_empty() {
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            // Find the root entry for this selection
            if let Some(idx) = self
                .document
                .roots
                .iter()
                .position(|e| e.root == selected_id)
            {
                let old_root = self.document.roots[idx].root;
                let new_id = self.alloc_node_id();

                // Create a translate node wrapping the old root
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: self
                            .document
                            .nodes
                            .get(&old_root)
                            .and_then(|n| n.name.clone()),
                        op: CsgOp::Translate {
                            child: old_root,
                            offset: Vec3::new(dx, dy, dz),
                        },
                    },
                );

                // Update the root entry
                self.document.roots[idx].root = new_id;

                // Update selection
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Translated by ({}, {}, {})", dx, dy, dz);
        Ok(())
    }

    /// Save the document to file.
    pub fn save(&mut self) -> Result<()> {
        if let Some(ref path) = self.file_path {
            let json = self.document.to_json()?;
            std::fs::write(path, json)?;
            self.status = format!("Saved to {}", path.display());
        } else {
            self.status = "No file path - use 'save <path>' command".to_string();
        }
        Ok(())
    }

    /// Save the document to a new file.
    pub fn save_as(&mut self, path: PathBuf) -> Result<()> {
        let json = self.document.to_json()?;
        std::fs::write(&path, json)?;
        self.file_path = Some(path.clone());
        self.status = format!("Saved to {}", path.display());
        Ok(())
    }

    /// Export to STL.
    pub fn export_stl(&self, path: &PathBuf) -> Result<()> {
        let mut combined_verts = Vec::new();
        let mut combined_idxs = Vec::new();
        for mesh in &self.meshes {
            let base_idx = (combined_verts.len() / 3) as u32;
            combined_verts.extend_from_slice(&mesh.vertices);
            for idx in &mesh.indices {
                combined_idxs.push(idx + base_idx);
            }
        }
        let stl_bytes = crate::export_stl_bytes(&combined_verts, &combined_idxs)?;
        std::fs::write(path, stl_bytes)?;
        Ok(())
    }

    /// Evaluate the document to get meshes.
    pub fn evaluate(&mut self) -> Result<()> {
        self.meshes = evaluate_document(&self.document)?;
        Ok(())
    }

    /// Get triangles for rendering.
    pub fn get_triangles(&self) -> Vec<Triangle> {
        let mut triangles = Vec::new();
        let color = [180u8, 180, 190];

        for mesh in &self.meshes {
            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }

                // Use checked arithmetic to prevent overflow and get() for bounds safety
                let Some(i0) = (tri[0] as usize).checked_mul(3) else {
                    continue;
                };
                let Some(i1) = (tri[1] as usize).checked_mul(3) else {
                    continue;
                };
                let Some(i2) = (tri[2] as usize).checked_mul(3) else {
                    continue;
                };

                // Use get() for bounds-checked access
                let (Some(&v0x), Some(&v0y), Some(&v0z)) = (
                    mesh.vertices.get(i0),
                    mesh.vertices.get(i0 + 1),
                    mesh.vertices.get(i0 + 2),
                ) else {
                    continue;
                };
                let (Some(&v1x), Some(&v1y), Some(&v1z)) = (
                    mesh.vertices.get(i1),
                    mesh.vertices.get(i1 + 1),
                    mesh.vertices.get(i1 + 2),
                ) else {
                    continue;
                };
                let (Some(&v2x), Some(&v2y), Some(&v2z)) = (
                    mesh.vertices.get(i2),
                    mesh.vertices.get(i2 + 1),
                    mesh.vertices.get(i2 + 2),
                ) else {
                    continue;
                };

                triangles.push(Triangle {
                    v0: [v0x, v0y, v0z],
                    v1: [v1x, v1y, v1z],
                    v2: [v2x, v2y, v2z],
                    color,
                });
            }
        }

        triangles
    }

    /// Check if currently in command input mode.
    pub fn command_mode(&self) -> bool {
        matches!(self.mode, TuiMode::Command)
    }

    /// Get the list of parts (scene entries) for the tree view.
    pub fn get_parts(&self) -> Vec<(NodeId, String)> {
        self.document
            .roots
            .iter()
            .map(|e| {
                let name = self
                    .document
                    .nodes
                    .get(&e.root)
                    .and_then(|n| n.name.clone())
                    .unwrap_or_else(|| format!("Node {}", e.root));
                (e.root, name)
            })
            .collect()
    }

    /// Process a command string.
    pub fn process_command(&mut self, cmd: &str) -> Result<()> {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return Ok(());
        }

        match parts[0].to_lowercase().as_str() {
            "cube" | "box" => {
                let size = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(20.0);
                self.add_cube(size)?;
            }
            "cylinder" | "cyl" => {
                let radius = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let height = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(20.0);
                self.add_cylinder(radius, height)?;
            }
            "sphere" => {
                let radius = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                self.add_sphere(radius)?;
            }
            "delete" | "del" | "rm" => {
                self.delete_selected()?;
            }
            "move" | "translate" => {
                let dx = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let dy = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let dz = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                self.translate_selected(dx, dy, dz)?;
            }
            "save" => {
                if let Some(path) = parts.get(1) {
                    self.save_as(PathBuf::from(path))?;
                } else {
                    self.save()?;
                }
            }
            "export" => {
                if let Some(path) = parts.get(1) {
                    let path = PathBuf::from(path);
                    self.export_stl(&path)?;
                    self.status = format!("Exported to {}", path.display());
                } else {
                    self.status = "Usage: export <path.stl>".to_string();
                }
            }
            "quit" | "q" => {
                self.running = false;
            }
            "help" | "?" => {
                self.status = "Commands: cube, cylinder, sphere, delete, move, save, export, quit"
                    .to_string();
            }
            _ => {
                self.status = format!("Unknown command: {}", parts[0]);
            }
        }

        Ok(())
    }
}

/// Evaluate a document to meshes.
pub fn evaluate_document(doc: &Document) -> Result<Vec<EvaluatedMesh>> {
    let mut meshes = Vec::new();

    for entry in &doc.roots {
        if let Some(solid) = evaluate_node(doc, entry.root)? {
            let mesh = solid.to_mesh(32);
            meshes.push(EvaluatedMesh {
                vertices: mesh.vertices,
                indices: mesh.indices,
            });
        }
    }

    Ok(meshes)
}

/// Recursively evaluate a node to a Solid.
fn evaluate_node(doc: &Document, node_id: NodeId) -> Result<Option<vcad_kernel::Solid>> {
    use vcad_kernel::Solid;

    let node = doc
        .nodes
        .get(&node_id)
        .ok_or_else(|| anyhow::anyhow!("Node {} not found", node_id))?;

    let solid = match &node.op {
        CsgOp::Empty => Some(Solid::empty()),
        CsgOp::Cube { size } => Some(Solid::cube(size.x, size.y, size.z)),
        CsgOp::Cylinder {
            radius,
            height,
            segments,
        } => Some(Solid::cylinder(*radius, *height, *segments)),
        CsgOp::Sphere { radius, segments } => Some(Solid::sphere(*radius, *segments)),
        CsgOp::Cone {
            radius_bottom,
            radius_top,
            height,
            segments,
        } => Some(Solid::cone(*radius_bottom, *radius_top, *height, *segments)),
        CsgOp::Union { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.union(&r)),
                (Some(l), None) => Some(l),
                (None, Some(r)) => Some(r),
                (None, None) => None,
            }
        }
        CsgOp::Difference { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.difference(&r)),
                (Some(l), None) => Some(l),
                _ => None,
            }
        }
        CsgOp::Intersection { left, right } => {
            let l = evaluate_node(doc, *left)?;
            let r = evaluate_node(doc, *right)?;
            match (l, r) {
                (Some(l), Some(r)) => Some(l.intersection(&r)),
                _ => None,
            }
        }
        CsgOp::Translate { child, offset } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.translate(offset.x, offset.y, offset.z))
        }
        CsgOp::Rotate { child, angles } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.rotate(angles.x, angles.y, angles.z))
        }
        CsgOp::Scale { child, factor } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.scale(factor.x, factor.y, factor.z))
        }
        CsgOp::Sketch2D { .. } => {
            // Sketches need extrusion to become solids
            None
        }
        CsgOp::Extrude { .. } => {
            // TODO: Implement sketch extrusion
            None
        }
        CsgOp::Revolve { .. } => {
            // TODO: Implement sketch revolve
            None
        }
        CsgOp::LinearPattern {
            child,
            direction,
            count,
            spacing,
        } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| {
                s.linear_pattern(
                    vcad_kernel::vcad_kernel_math::Vec3::new(direction.x, direction.y, direction.z),
                    *count,
                    *spacing,
                )
            })
        }
        CsgOp::CircularPattern {
            child,
            axis_origin,
            axis_dir,
            count,
            angle_deg,
        } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| {
                s.circular_pattern(
                    vcad_kernel::vcad_kernel_math::Point3::new(
                        axis_origin.x,
                        axis_origin.y,
                        axis_origin.z,
                    ),
                    vcad_kernel::vcad_kernel_math::Vec3::new(axis_dir.x, axis_dir.y, axis_dir.z),
                    *count,
                    *angle_deg,
                )
            })
        }
        CsgOp::Shell { child, thickness } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.shell(*thickness))
        }
        CsgOp::Fillet { child, radius } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.fillet(*radius))
        }
        CsgOp::Chamfer { child, distance } => {
            let c = evaluate_node(doc, *child)?;
            c.map(|s| s.chamfer(*distance))
        }
        CsgOp::StepImport { path } => {
            // Import geometry from STEP file
            match Solid::from_step(path) {
                Ok(solid) => Some(solid),
                Err(e) => {
                    eprintln!("Failed to import STEP file '{}': {}", path, e);
                    None
                }
            }
        }
        CsgOp::Text2D { .. } => {
            // Text needs extrusion to become solid
            None
        }
    };

    Ok(solid)
}

/// Run the TUI application.
pub fn run_tui(file: Option<PathBuf>) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Create app
    let mut app = App::new(file)?;

    // Main loop
    let result = run_loop(&mut terminal, &mut app);

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    result
}

fn run_loop(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    let mut render_buffer = RenderBuffer::new(80, 40);
    let mut focused_part_index: usize = 0;

    while app.running {
        // Get terminal size
        let size = terminal.size()?;

        // Update render buffer size based on viewport
        let viewport_width = (size.width.saturating_sub(28)) as u32 * 2; // 2 pixels per char width
        let viewport_height = (size.height.saturating_sub(6)) as u32 * 4; // 4 pixels per char height (braille)
        if render_buffer.width != viewport_width || render_buffer.height != viewport_height {
            render_buffer = RenderBuffer::new(viewport_width.max(40), viewport_height.max(20));
        }

        // Render 3D scene to buffer
        let triangles = app.get_triangles();
        crate::render::render_scene(&mut render_buffer, &triangles, &app.camera);

        // Draw UI
        terminal.draw(|f| {
            ui::draw(f, app, &render_buffer, focused_part_index);
        })?;

        // Handle input
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                match &mut app.mode {
                    TuiMode::Command => {
                        // Command input mode
                        match key.code {
                            KeyCode::Enter => {
                                let cmd = app.command_input.clone();
                                app.command_input.clear();
                                app.mode = TuiMode::Normal;
                                if let Err(e) = app.process_command(&cmd) {
                                    app.status = format!("Error: {}", e);
                                }
                            }
                            KeyCode::Esc => {
                                app.command_input.clear();
                                app.mode = TuiMode::Normal;
                            }
                            KeyCode::Backspace => {
                                app.command_input.pop();
                            }
                            KeyCode::Char(c) => {
                                app.command_input.push(c);
                            }
                            _ => {}
                        }
                    }
                    TuiMode::Sketch(state) => {
                        // Sketch mode
                        match key.code {
                            KeyCode::Esc => {
                                app.mode = TuiMode::Normal;
                                app.status = "Exited sketch mode".to_string();
                            }
                            KeyCode::Char(c) => {
                                if state.handle_key(c) {
                                    app.status = format!("Sketch tool: {}", state.tool_name());
                                }
                            }
                            _ => {}
                        }
                    }
                    TuiMode::Normal => {
                        // Normal mode
                        match key.code {
                            KeyCode::Char('q') => {
                                app.running = false;
                            }
                            KeyCode::Char(':') | KeyCode::Char('/') => {
                                app.mode = TuiMode::Command;
                            }
                            // Enter sketch mode
                            KeyCode::Char('S') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                                app.mode = TuiMode::Sketch(crate::tui::SketchModeState::new(SketchPlane::XY));
                                app.status = "Sketch mode (XY plane) - L:line R:rect C:circle".to_string();
                            }
                            KeyCode::Char('1') => {
                                let id = app.add_cube(20.0)?;
                                app.selected.clear();
                                app.selected.insert(id);
                            }
                            KeyCode::Char('2') => {
                                let id = app.add_cylinder(10.0, 20.0)?;
                                app.selected.clear();
                                app.selected.insert(id);
                            }
                            KeyCode::Char('3') => {
                                let id = app.add_sphere(10.0)?;
                                app.selected.clear();
                                app.selected.insert(id);
                            }
                            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                                app.delete_selected()?;
                            }
                            KeyCode::Char('u') => {
                                app.undo()?;
                            }
                            KeyCode::Char('r') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.redo()?;
                            }
                            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.save()?;
                            }
                            // Camera rotation
                            KeyCode::Left => {
                                app.camera.rotate_horizontal(-15.0);
                            }
                            KeyCode::Right => {
                                app.camera.rotate_horizontal(15.0);
                            }
                            KeyCode::Up => {
                                app.camera.rotate_vertical(15.0);
                            }
                            KeyCode::Down => {
                                app.camera.rotate_vertical(-15.0);
                            }
                            // Zoom
                            KeyCode::Char('+') | KeyCode::Char('=') => {
                                app.camera.zoom(0.8);
                            }
                            KeyCode::Char('-') => {
                                app.camera.zoom(1.25);
                            }
                            // Part selection
                            KeyCode::Tab => {
                                let parts = app.get_parts();
                                if !parts.is_empty() {
                                    focused_part_index = (focused_part_index + 1) % parts.len();
                                    app.selected.clear();
                                    app.selected.insert(parts[focused_part_index].0);
                                }
                            }
                            KeyCode::Esc => {
                                app.selected.clear();
                            }
                            KeyCode::Enter => {
                                let parts = app.get_parts();
                                if focused_part_index < parts.len() {
                                    let id = parts[focused_part_index].0;
                                    if app.selected.contains(&id) {
                                        app.selected.remove(&id);
                                    } else {
                                        app.selected.insert(id);
                                    }
                                }
                            }
                            // WASD for translation
                            KeyCode::Char('w') => {
                                app.translate_selected(0.0, 0.0, 5.0)?;
                            }
                            KeyCode::Char('s') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                                app.translate_selected(0.0, 0.0, -5.0)?;
                            }
                            KeyCode::Char('a') => {
                                app.translate_selected(-5.0, 0.0, 0.0)?;
                            }
                            KeyCode::Char('d') => {
                                app.translate_selected(5.0, 0.0, 0.0)?;
                            }
                            _ => {}
                        }
                    }
                    // Other modes - just handle Esc to exit for now
                    _ => {
                        if key.code == KeyCode::Esc {
                            app.mode = TuiMode::Normal;
                            app.status = "Ready".to_string();
                        }
                    }
                }
            }
        }
    }

    Ok(())
}
