//! TUI application state and main loop.

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use std::{
    collections::HashSet,
    io::{self, Stdout},
    path::PathBuf,
    time::Duration,
};
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

use crate::input::{ClickTracker, DragState};
use crate::render::{Camera, GraphicsOutput, GraphicsProtocol, RenderBuffer, Triangle};
use crate::tui::TuiMode;
use crate::ui;
use crate::ui::buffer::{CellBuffer, Rect};

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
    /// Selected index in command palette.
    pub command_selected_index: usize,
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

    // -- Visual state --
    /// Whether the sidebar is visible.
    pub sidebar_visible: bool,
    /// Whether the camera is being orbited (hides overlays).
    pub is_orbiting: bool,
    /// Active toolbar tab index (0-7).
    pub active_tab: usize,
    /// Sidebar scroll offset.
    pub sidebar_scroll: usize,
    /// Focused part index in sidebar.
    pub focused_part_index: usize,

    // -- Mouse state --
    /// Current drag operation.
    pub drag: DragState,
    /// Current mouse position (col, row).
    pub mouse_pos: (u16, u16),
    /// Double-click tracker.
    pub click_tracker: ClickTracker,
    /// Whether the 3D scene needs re-rendering.
    pub render_dirty: bool,
    /// Whether to use CPU ray tracing instead of rasterizer.
    pub raytrace_enabled: bool,
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
            command_selected_index: 0,
            status: "Ready".to_string(),
            meshes: Vec::new(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            next_node_id,
            file_path,
            // Visual state
            sidebar_visible: true,
            is_orbiting: false,
            active_tab: 1, // Create tab
            sidebar_scroll: 0,
            focused_part_index: 0,
            // Mouse state
            drag: DragState::default(),
            mouse_pos: (0, 0),
            click_tracker: ClickTracker::default(),
            render_dirty: true,
            raytrace_enabled: false,
        };

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

        self.document
            .roots
            .retain(|e| !self.selected.contains(&e.root));

        for &id in &self.selected {
            self.document.nodes.remove(&id);
        }

        let count = self.selected.len();
        self.selected.clear();
        self.evaluate()?;
        self.status = format!("Deleted {} part(s)", count);
        Ok(())
    }

    /// Add a cone primitive.
    pub fn add_cone(&mut self, radius: f64, height: f64) -> Result<NodeId> {
        self.push_undo();
        let id = self.alloc_node_id();
        self.document.nodes.insert(
            id,
            Node {
                id,
                name: Some(format!("Cone {}", id)),
                op: CsgOp::Cone {
                    radius_bottom: radius,
                    radius_top: 0.0,
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
        self.status = format!("Added cone {}", id);
        Ok(id)
    }

    /// Boolean union of selected nodes (requires 2+).
    pub fn boolean_union(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.status = "Select 2+ parts for union".to_string();
            return Ok(());
        }
        self.push_undo();

        // Chain union: fold left
        let mut result_id = ids[0];
        for &other_id in &ids[1..] {
            let new_id = self.alloc_node_id();
            self.document.nodes.insert(
                new_id,
                Node {
                    id: new_id,
                    name: Some(format!("Union {}", new_id)),
                    op: CsgOp::Union {
                        left: result_id,
                        right: other_id,
                    },
                },
            );
            // Remove the consumed roots
            self.document.roots.retain(|e| e.root != other_id);
            if let Some(entry) = self.document.roots.iter_mut().find(|e| e.root == result_id) {
                entry.root = new_id;
            }
            result_id = new_id;
        }

        self.selected.clear();
        self.selected.insert(result_id);
        self.evaluate()?;
        self.status = "Union applied".to_string();
        Ok(())
    }

    /// Boolean difference of selected nodes (first - rest).
    pub fn boolean_difference(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.status = "Select 2+ parts for difference".to_string();
            return Ok(());
        }
        self.push_undo();

        let mut result_id = ids[0];
        for &other_id in &ids[1..] {
            let new_id = self.alloc_node_id();
            self.document.nodes.insert(
                new_id,
                Node {
                    id: new_id,
                    name: Some(format!("Difference {}", new_id)),
                    op: CsgOp::Difference {
                        left: result_id,
                        right: other_id,
                    },
                },
            );
            self.document.roots.retain(|e| e.root != other_id);
            if let Some(entry) = self.document.roots.iter_mut().find(|e| e.root == result_id) {
                entry.root = new_id;
            }
            result_id = new_id;
        }

        self.selected.clear();
        self.selected.insert(result_id);
        self.evaluate()?;
        self.status = "Difference applied".to_string();
        Ok(())
    }

    /// Boolean intersection of selected nodes.
    pub fn boolean_intersection(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.status = "Select 2+ parts for intersection".to_string();
            return Ok(());
        }
        self.push_undo();

        let mut result_id = ids[0];
        for &other_id in &ids[1..] {
            let new_id = self.alloc_node_id();
            self.document.nodes.insert(
                new_id,
                Node {
                    id: new_id,
                    name: Some(format!("Intersection {}", new_id)),
                    op: CsgOp::Intersection {
                        left: result_id,
                        right: other_id,
                    },
                },
            );
            self.document.roots.retain(|e| e.root != other_id);
            if let Some(entry) = self.document.roots.iter_mut().find(|e| e.root == result_id) {
                entry.root = new_id;
            }
            result_id = new_id;
        }

        self.selected.clear();
        self.selected.insert(result_id);
        self.evaluate()?;
        self.status = "Intersection applied".to_string();
        Ok(())
    }

    /// Fillet all edges of selected nodes.
    pub fn fillet_selected(&mut self, radius: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.status = "Select a part to fillet".to_string();
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Fillet {}", new_id)),
                        op: CsgOp::Fillet {
                            child: selected_id,
                            radius,
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Fillet r={:.1} applied", radius);
        Ok(())
    }

    /// Chamfer all edges of selected nodes.
    pub fn chamfer_selected(&mut self, distance: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.status = "Select a part to chamfer".to_string();
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Chamfer {}", new_id)),
                        op: CsgOp::Chamfer {
                            child: selected_id,
                            distance,
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Chamfer d={:.1} applied", distance);
        Ok(())
    }

    /// Shell selected nodes.
    pub fn shell_selected(&mut self, thickness: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.status = "Select a part to shell".to_string();
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Shell {}", new_id)),
                        op: CsgOp::Shell {
                            child: selected_id,
                            thickness,
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Shell t={:.1} applied", thickness);
        Ok(())
    }

    /// Linear pattern of selected nodes.
    pub fn pattern_selected(&mut self) -> Result<()> {
        if self.selected.is_empty() {
            self.status = "Select a part to pattern".to_string();
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Pattern {}", new_id)),
                        op: CsgOp::LinearPattern {
                            child: selected_id,
                            direction: Vec3::new(25.0, 0.0, 0.0),
                            count: 3,
                            spacing: 25.0,
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = "Linear pattern applied".to_string();
        Ok(())
    }

    /// Mirror selected nodes along X axis.
    pub fn mirror_selected(&mut self) -> Result<()> {
        if self.selected.is_empty() {
            self.status = "Select a part to mirror".to_string();
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Mirror {}", new_id)),
                        op: CsgOp::Scale {
                            child: selected_id,
                            factor: Vec3::new(-1.0, 1.0, 1.0),
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = "Mirror applied".to_string();
        Ok(())
    }

    /// Rotate selected nodes.
    pub fn rotate_selected(&mut self, rx: f64, ry: f64, rz: f64) -> Result<()> {
        if self.selected.is_empty() {
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: self
                            .document
                            .nodes
                            .get(&selected_id)
                            .and_then(|n| n.name.clone()),
                        op: CsgOp::Rotate {
                            child: selected_id,
                            angles: Vec3::new(rx, ry, rz),
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Rotated by ({}, {}, {})", rx, ry, rz);
        Ok(())
    }

    /// Scale selected nodes.
    pub fn scale_selected(&mut self, sx: f64, sy: f64, sz: f64) -> Result<()> {
        if self.selected.is_empty() {
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self.document.roots.iter().position(|e| e.root == selected_id) {
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: self
                            .document
                            .nodes
                            .get(&selected_id)
                            .and_then(|n| n.name.clone()),
                        op: CsgOp::Scale {
                            child: selected_id,
                            factor: Vec3::new(sx, sy, sz),
                        },
                    },
                );
                self.document.roots[idx].root = new_id;
                self.selected.remove(&selected_id);
                self.selected.insert(new_id);
            }
        }

        self.evaluate()?;
        self.status = format!("Scaled by ({}, {}, {})", sx, sy, sz);
        Ok(())
    }

    /// Translate selected nodes.
    pub fn translate_selected(&mut self, dx: f64, dy: f64, dz: f64) -> Result<()> {
        if self.selected.is_empty() {
            return Ok(());
        }
        self.push_undo();

        for &selected_id in &self.selected.clone() {
            if let Some(idx) = self
                .document
                .roots
                .iter()
                .position(|e| e.root == selected_id)
            {
                let old_root = self.document.roots[idx].root;
                let new_id = self.alloc_node_id();

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

                self.document.roots[idx].root = new_id;
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
        self.render_dirty = true;
        Ok(())
    }

    /// Get triangles for rendering, with pick IDs per mesh.
    pub fn get_triangles(&self) -> Vec<Triangle> {
        let mut triangles = Vec::new();
        let color = [180u8, 180, 190];

        for (mesh_idx, mesh) in self.meshes.iter().enumerate() {
            let pick_id = if mesh_idx < self.document.roots.len() {
                self.document.roots[mesh_idx].root as u32
            } else {
                (mesh_idx + 1) as u32
            };

            let mesh_color = if self.selected.contains(&(pick_id as u64)) {
                [220u8, 100, 140]
            } else {
                color
            };

            for tri in mesh.indices.chunks(3) {
                if tri.len() < 3 {
                    continue;
                }

                let Some(i0) = (tri[0] as usize).checked_mul(3) else {
                    continue;
                };
                let Some(i1) = (tri[1] as usize).checked_mul(3) else {
                    continue;
                };
                let Some(i2) = (tri[2] as usize).checked_mul(3) else {
                    continue;
                };

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
                    color: mesh_color,
                    pick_id,
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
            "cube" | "box" | "add cube" => {
                let size = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(20.0);
                self.add_cube(size)?;
            }
            "cylinder" | "cyl" | "add cylinder" => {
                let radius = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let height = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(20.0);
                self.add_cylinder(radius, height)?;
            }
            "sphere" | "add sphere" => {
                let radius = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                self.add_sphere(radius)?;
            }
            "cone" | "add cone" => {
                let radius = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(10.0);
                let height = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(20.0);
                self.add_cone(radius, height)?;
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
            "rotate" => {
                let rx = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let ry = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                let rz = parts.get(3).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                self.rotate_selected(rx, ry, rz)?;
            }
            "scale" => {
                let s = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
                self.scale_selected(s, s, s)?;
            }
            "union" => self.boolean_union()?,
            "difference" | "subtract" => self.boolean_difference()?,
            "intersection" | "intersect" => self.boolean_intersection()?,
            "fillet" => {
                let r = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
                self.fillet_selected(r)?;
            }
            "chamfer" => {
                let d = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(2.0);
                self.chamfer_selected(d)?;
            }
            "shell" => {
                let t = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(1.0);
                self.shell_selected(t)?;
            }
            "pattern" => self.pattern_selected()?,
            "mirror" => self.mirror_selected()?,
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
            "undo" => self.undo()?,
            "redo" => self.redo()?,
            "quit" | "q" => {
                self.running = false;
            }
            "help" | "?" => {
                self.status =
                    "Commands: cube, cylinder, sphere, delete, move, save, export, quit".to_string();
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
        CsgOp::Sketch2D { .. } => None,
        CsgOp::Extrude { .. } => None,
        CsgOp::Revolve { .. } => None,
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
        CsgOp::StepImport { path } => match Solid::from_step(path) {
            Ok(solid) => Some(solid),
            Err(e) => {
                eprintln!("Failed to import STEP file '{}': {}", path, e);
                None
            }
        },
        CsgOp::Text2D { .. } => None,
    };

    Ok(solid)
}

/// Run the TUI application.
pub fn run_tui(file: Option<PathBuf>) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;

    let mut app = App::new(file)?;

    let result = run_loop(&mut stdout, &mut app);

    disable_raw_mode()?;
    execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    )?;

    result
}

fn run_loop(stdout: &mut Stdout, app: &mut App) -> Result<()> {
    let (term_w, term_h) = terminal::size()?;
    let mut cell_buffer = CellBuffer::new(term_w, term_h);
    let mut render_buffer = RenderBuffer::new(80, 40);
    let mut last_camera = app.camera.snapshot();
    let mut gfx = GraphicsOutput::new();
    let protocol = gfx.protocol();

    while app.running {
        let (term_w, term_h) = terminal::size()?;
        let area = Rect::new(0, 0, term_w, term_h);

        // Resize cell buffer if needed
        if cell_buffer.width != term_w || cell_buffer.height != term_h {
            cell_buffer.resize(term_w, term_h);
            app.render_dirty = true;
        }

        // Size render buffer based on protocol
        let (viewport_width, viewport_height) = match protocol {
            GraphicsProtocol::Kitty | GraphicsProtocol::ITerm2 | GraphicsProtocol::Sixel => {
                let caps = gfx.caps();
                let w = area.width as u32 * caps.cell_width;
                let h = area.height as u32 * caps.cell_height;
                (w, h)
            }
            GraphicsProtocol::HalfBlock => {
                (area.width as u32, (area.height as u32) * 2)
            }
            GraphicsProtocol::Braille => {
                ((area.width as u32) * 2, (area.height as u32) * 4)
            }
        };

        if render_buffer.width != viewport_width || render_buffer.height != viewport_height {
            render_buffer = RenderBuffer::new(viewport_width.max(10), viewport_height.max(10));
            app.render_dirty = true;
        }

        // Only re-render 3D scene when something changed
        let current_camera = app.camera.snapshot();
        let viewport_dirty = app.render_dirty || current_camera != last_camera;
        if viewport_dirty {
            if app.raytrace_enabled {
                render_raytrace(app, &mut render_buffer);
            } else {
                let triangles = app.get_triangles();
                render_buffer.clear(0x22, 0x22, 0x22);
                crate::render::render_scene(&mut render_buffer, &triangles, &app.camera);
            }
            last_camera = current_camera;
            app.render_dirty = false;
        }

        // Branch viewport rendering on protocol
        match protocol {
            GraphicsProtocol::Kitty | GraphicsProtocol::ITerm2 | GraphicsProtocol::Sixel => {
                if viewport_dirty {
                    // Move cursor to top-left and output pixel-perfect image
                    execute!(stdout, crossterm::cursor::MoveTo(0, 0))?;
                    gfx.display(&render_buffer, stdout)?;
                }
                // Overlay UI via CellBuffer on top
                ui::draw_overlays(&mut cell_buffer, app);
            }
            GraphicsProtocol::HalfBlock => {
                ui::draw(&mut cell_buffer, app, &render_buffer);
            }
            GraphicsProtocol::Braille => {
                ui::draw_braille(&mut cell_buffer, app, &render_buffer);
            }
        }

        // Flush only changed cells
        cell_buffer.flush(stdout)?;

        // Handle input with 16ms poll
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) => {
                    if !crate::input::handle_key(app, key)? {
                        app.running = false;
                    }
                }
                Event::Mouse(mouse) => {
                    let cell_dims = match protocol {
                        GraphicsProtocol::Kitty
                        | GraphicsProtocol::ITerm2
                        | GraphicsProtocol::Sixel => {
                            let caps = gfx.caps();
                            Some((caps.cell_width, caps.cell_height))
                        }
                        _ => None,
                    };
                    crate::input::handle_mouse(
                        app,
                        mouse,
                        area,
                        &render_buffer,
                        cell_dims,
                    )?;
                }
                Event::Resize(_, _) => {
                    // Terminal resized — will be handled on next loop iteration
                }
                _ => {}
            }
        }
    }

    Ok(())
}

/// Render the scene using CPU ray tracing and copy into the RenderBuffer.
fn render_raytrace(app: &App, buffer: &mut RenderBuffer) {
    use std::sync::Arc;
    use vcad_kernel_raytrace::cpu::render_scene as rt_render;

    // Evaluate BRep solids from the document
    let mut solids = Vec::new();
    let mut colors_vec = Vec::new();

    for entry in &app.document.roots {
        if let Ok(Some(solid)) = evaluate_node(&app.document, entry.root) {
            if let Some(brep) = solid.brep() {
                solids.push(Arc::new(brep.clone()));
                let pick_id = entry.root as u32;
                let is_selected = app.selected.contains(&(pick_id as u64));
                if is_selected {
                    colors_vec.extend_from_slice(&[0.86f32, 0.39, 0.55]);
                } else {
                    colors_vec.extend_from_slice(&[0.71f32, 0.71, 0.75]);
                }
            }
        }
    }

    if solids.is_empty() {
        buffer.clear(30, 30, 35);
        return;
    }

    // Camera parameters
    let cam = &app.camera;
    let camera_pos = vcad_kernel::vcad_kernel_math::Point3::new(
        cam.position.x as f64,
        cam.position.y as f64,
        cam.position.z as f64,
    );
    let target_pos = vcad_kernel::vcad_kernel_math::Point3::new(
        cam.target.x as f64,
        cam.target.y as f64,
        cam.target.z as f64,
    );
    let up = vcad_kernel::vcad_kernel_math::Vec3::new(0.0, 1.0, 0.0)
        .try_normalize(1e-10)
        .unwrap_or(vcad_kernel::vcad_kernel_math::Vec3::y_axis().into_inner());
    let up_dir = vcad_kernel::vcad_kernel_math::Dir3::new_normalize(up);

    // Identity transforms for all solids
    let transforms: Vec<f64> = solids
        .iter()
        .flat_map(|_| {
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0,
                1.0,
            ]
        })
        .collect();

    let pixels = rt_render(
        &solids,
        &transforms,
        &colors_vec,
        camera_pos,
        target_pos,
        up_dir,
        buffer.width,
        buffer.height,
        cam.fov as f64,
    );

    // Copy ray-traced RGBA into render buffer
    let size = (buffer.width * buffer.height) as usize;
    if pixels.len() >= size * 4 {
        buffer.pixels[..size * 4].copy_from_slice(&pixels[..size * 4]);
    }
    // Clear pick_ids (ray tracing doesn't populate them yet)
    for id in &mut buffer.pick_ids {
        *id = 0;
    }
    for d in &mut buffer.depth {
        *d = f32::INFINITY;
    }
}
