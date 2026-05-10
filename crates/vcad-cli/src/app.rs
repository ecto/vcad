//! TUI application state and main loop.

use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{
        self, disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use std::{
    collections::{HashSet, VecDeque},
    io::{self, Stdout},
    path::PathBuf,
    time::{Duration, Instant},
};
use vcad_crdt::{CrdtDocument, ReplicaId};
use vcad_ir::{CsgOp, Document, Node, NodeId, SceneEntry, Vec3};

use crate::input::{ClickTracker, DragState};
use crate::render::{Camera, GraphicsOutput, GraphicsProtocol, RenderBuffer, Triangle};
use crate::tui::TuiMode;
use crate::ui;
use crate::ui::buffer::{CellBuffer, Rect};
use crate::ui::chat::ChatPanel;
use crate::ui::toolbar::ToolInput;

/// Mesh data from evaluation.
pub struct EvaluatedMesh {
    pub vertices: Vec<f32>,
    pub indices: Vec<u32>,
}

/// Severity of a log line — maps to the DEBUG/INFO/WARN/ERROR pills shown
/// in the status bar ticker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    #[allow(dead_code)]
    Debug,
    Info,
    #[allow(dead_code)]
    Warn,
    #[allow(dead_code)]
    Error,
}

impl LogLevel {
    /// Short uppercase label for the status ticker.
    pub fn label(self) -> &'static str {
        match self {
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warn => "WARN",
            LogLevel::Error => "ERROR",
        }
    }
}

/// Structured log entry pushed by `App::log` and rendered by the status bar.
#[derive(Debug, Clone)]
pub struct LogEntry {
    pub level: LogLevel,
    pub source: &'static str,
    pub message: String,
    pub timestamp: Instant,
}

/// Max log entries retained in the in-memory ring buffer.
const MAX_LOG_ENTRIES: usize = 200;
/// Max characters stored per log message — cuts off gigantic HTTP error
/// bodies before they reach the status bar.
const MAX_LOG_MESSAGE_LEN: usize = 500;

/// Normalize a raw log string: collapse whitespace/control runs into a
/// single space and truncate to `MAX_LOG_MESSAGE_LEN` characters. This is
/// the ingestion-side guard; `ui/buffer.rs::set_char` has a second
/// defense that turns any control char that slips through into `·`.
fn normalize_log_message(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len().min(MAX_LOG_MESSAGE_LEN));
    let mut last_was_space = false;
    for ch in raw.chars() {
        if out.chars().count() >= MAX_LOG_MESSAGE_LEN {
            out.push('…');
            break;
        }
        let is_space_like = ch.is_whitespace() || (ch as u32) < 0x20 || (ch as u32) == 0x7F;
        if is_space_like {
            if !last_was_space {
                out.push(' ');
                last_was_space = true;
            }
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    out.trim().to_string()
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
    /// CRDT document for new-style operations.
    #[allow(dead_code)] // staged for future CRDT-backed ops
    pub crdt: CrdtDocument,

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
    /// Drop-down chat panel.
    pub chat: ChatPanel,
    /// Active inline parameter input (replaces sub-tool row when Some).
    pub tool_input: Option<ToolInput>,
    /// Timestamp of last manual tab click (suppresses auto-switch briefly).
    pub last_manual_tab: Instant,
    /// Menu-bar dropdown state (which top-level menu is open).
    pub menu_state: crate::ui::menu::MenuBarState,
    /// True after any edit until the next save — drives the ● indicator.
    dirty: bool,
    /// Ring buffer of structured log entries powering the status-bar ticker.
    pub logs: VecDeque<LogEntry>,
    /// Most recent cursor world-space position (mm, Z-up), if the cursor is
    /// hovering over the viewport. Populated by input hit-testing in M2;
    /// currently `None` so the middle segment shows `—` placeholders.
    pub cursor_world: Option<(f64, f64, f64)>,
    /// Conversational chat state — message history, in-flight request
    /// receiver, accumulating assistant buffer. Populated by chat_session.
    pub chat_session: crate::chat_session::ChatSession,
    /// Shared command/keybinding registry — same instance type the web app
    /// uses through wasm. Drives `handle_key`'s dispatch path before the
    /// legacy match arms run.
    pub keybindings: vcad_app::KeybindingRegistry,

    // -- Welcome overlay --
    /// Show the welcome overlay on first launch with no file.
    pub show_welcome: bool,
    /// Currently highlighted item in the welcome overlay.
    pub welcome_selected: usize,
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
        let is_new_session = file_path.is_none();

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
            crdt: CrdtDocument::new(ReplicaId(1)),
            // Visual state
            sidebar_visible: true,
            is_orbiting: false,
            active_tab: 0, // Create tab
            sidebar_scroll: 0,
            focused_part_index: 0,
            // Mouse state
            drag: DragState::default(),
            mouse_pos: (0, 0),
            click_tracker: ClickTracker::default(),
            render_dirty: true,
            raytrace_enabled: false,
            chat: ChatPanel::new(),
            tool_input: None,
            last_manual_tab: Instant::now(),
            menu_state: Default::default(),
            dirty: false,
            logs: VecDeque::with_capacity(MAX_LOG_ENTRIES),
            cursor_world: None,
            chat_session: Default::default(),
            keybindings: vcad_app::KeybindingRegistry::new(),
            show_welcome: is_new_session,
            welcome_selected: 0,
        };

        // The TUI uses single bare keys for vim-style local actions
        // (`r`=redo, `7`=Export tab, `g` and `Shift+S` are unused). The
        // web registry binds `r`/`g`/`Shift+S` to `rotate`/`translate`/
        // `scale` (modal CAD style, has_selection-gated) and `7` to
        // `camera_iso`. Both styles are valid; on this host we want the
        // legacy match arms to keep handling those keys, so we clear the
        // registry's defaults for them. Bindings come back automatically
        // for any user that re-binds via the prefs UI.
        for cmd_id in [
            "camera_iso",
            "camera_top",
            "camera_front",
            "camera_right",
            "camera_fit",
            "translate",
            "rotate",
            "scale",
            "toggle_sidebar",
            "toggle_wireframe",
            "toggle_grid_snap",
            "toggle_devtools",
            "palette",
        ] {
            app.keybindings.set_binding(cmd_id, None);
        }

        // Rehydrate chat history so the sidebar shows prior turns on
        // launch. Full fidelity goes into session.messages (what the
        // model sees on the next turn); a simplified view goes into
        // chat.lines (what the user sees in the sidebar).
        let history = crate::chat_session::load_history();
        if !history.is_empty() {
            crate::chat_session::rehydrate_display(&mut app, &history);
            app.chat_session.messages = history;
        }

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
        self.dirty = true;
    }

    /// True while the document has unsaved changes.
    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Undo the last action.
    pub fn undo(&mut self) -> Result<()> {
        if let Some(prev) = self.undo_stack.pop() {
            self.redo_stack.push(self.document.clone());
            self.document = prev;
            self.evaluate()?;
            self.set_status("Undo");
        }
        Ok(())
    }

    /// Redo the last undone action.
    pub fn redo(&mut self) -> Result<()> {
        if let Some(next) = self.redo_stack.pop() {
            self.undo_stack.push(self.document.clone());
            self.document = next;
            self.evaluate()?;
            self.set_status("Redo");
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
        self.set_status(format!("Added cube {}", id));
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
        self.set_status(format!("Added cylinder {}", id));
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
        self.set_status(format!("Added sphere {}", id));
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
        self.set_status(format!("Deleted {} part(s)", count));
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
        self.set_status(format!("Added cone {}", id));
        Ok(id)
    }

    /// Boolean union of selected nodes (requires 2+).
    pub fn boolean_union(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.set_status("Select 2+ parts for union");
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
        self.set_status("Union applied");
        Ok(())
    }

    /// Boolean difference of selected nodes (first - rest).
    pub fn boolean_difference(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.set_status("Select 2+ parts for difference");
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
        self.set_status("Difference applied");
        Ok(())
    }

    /// Boolean intersection of selected nodes.
    pub fn boolean_intersection(&mut self) -> Result<()> {
        let ids: Vec<NodeId> = self.selected.iter().copied().collect();
        if ids.len() < 2 {
            self.set_status("Select 2+ parts for intersection");
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
        self.set_status("Intersection applied");
        Ok(())
    }

    /// Fillet all edges of selected nodes.
    pub fn fillet_selected(&mut self, radius: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.set_status("Select a part to fillet");
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
        self.set_status(format!("Fillet r={:.1} applied", radius));
        Ok(())
    }

    /// Chamfer all edges of selected nodes.
    pub fn chamfer_selected(&mut self, distance: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.set_status("Select a part to chamfer");
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
        self.set_status(format!("Chamfer d={:.1} applied", distance));
        Ok(())
    }

    /// Shell selected nodes.
    pub fn shell_selected(&mut self, thickness: f64) -> Result<()> {
        if self.selected.is_empty() {
            self.set_status("Select a part to shell");
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
        self.set_status(format!("Shell t={:.1} applied", thickness));
        Ok(())
    }

    /// Linear pattern of selected nodes.
    pub fn pattern_selected(&mut self, count: u32) -> Result<()> {
        if self.selected.is_empty() {
            self.set_status("Select a part to pattern");
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
                let new_id = self.alloc_node_id();
                self.document.nodes.insert(
                    new_id,
                    Node {
                        id: new_id,
                        name: Some(format!("Pattern {}", new_id)),
                        op: CsgOp::LinearPattern {
                            child: selected_id,
                            direction: Vec3::new(25.0, 0.0, 0.0),
                            count,
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
        self.set_status("Linear pattern applied");
        Ok(())
    }

    /// Mirror selected nodes along X axis.
    pub fn mirror_selected(&mut self) -> Result<()> {
        if self.selected.is_empty() {
            self.set_status("Select a part to mirror");
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
        self.set_status("Mirror applied");
        Ok(())
    }

    /// Rotate selected nodes.
    pub fn rotate_selected(&mut self, rx: f64, ry: f64, rz: f64) -> Result<()> {
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
        self.set_status(format!("Rotated by ({}, {}, {})", rx, ry, rz));
        Ok(())
    }

    /// Scale selected nodes.
    pub fn scale_selected(&mut self, sx: f64, sy: f64, sz: f64) -> Result<()> {
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
        self.set_status(format!("Scaled by ({}, {}, {})", sx, sy, sz));
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
        self.set_status(format!("Translated by ({}, {}, {})", dx, dy, dz));
        Ok(())
    }

    /// Save the document to file.
    pub fn save(&mut self) -> Result<()> {
        if let Some(ref path) = self.file_path {
            let json = self.document.to_json()?;
            std::fs::write(path, json)?;
            self.dirty = false;
            self.set_status(format!("Saved to {}", path.display()));
        } else {
            self.set_status("No file path - use 'save <path>' command");
        }
        Ok(())
    }

    /// Save the document to a new file.
    pub fn save_as(&mut self, path: PathBuf) -> Result<()> {
        let json = self.document.to_json()?;
        std::fs::write(&path, json)?;
        self.file_path = Some(path.clone());
        self.dirty = false;
        self.set_status(format!("Saved to {}", path.display()));
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

    /// Set status and log to the status-bar ticker + chat panel debug output.
    pub fn set_status(&mut self, msg: impl Into<String>) {
        let s: String = msg.into();
        self.status = s.clone();
        self.log(LogLevel::Info, "status", s.clone());
        self.chat.debug(s);
    }

    /// Push a structured log entry. Used by the status-bar ticker and, later,
    /// by the chat session + kernel error channel.
    ///
    /// Normalizes the message: collapses any run of whitespace/control
    /// characters into a single space and truncates to [`MAX_LOG_MESSAGE_LEN`]
    /// so a multiline HTTP 404 body doesn't blow up the ticker.
    pub fn log(&mut self, level: LogLevel, source: &'static str, message: impl Into<String>) {
        let raw = message.into();
        let normalized = normalize_log_message(&raw);
        self.logs.push_back(LogEntry {
            level,
            source,
            message: normalized,
            timestamp: Instant::now(),
        });
        while self.logs.len() > MAX_LOG_ENTRIES {
            self.logs.pop_front();
        }
    }

    /// Most recent log entry, if any.
    pub fn latest_log(&self) -> Option<&LogEntry> {
        self.logs.back()
    }

    /// Check if currently in command input mode.
    pub fn command_mode(&self) -> bool {
        matches!(self.mode, TuiMode::Command)
    }

    /// Auto-switch toolbar tab based on selection state (matches web app).
    pub fn auto_switch_tab(&mut self) {
        // Don't switch during inline parameter input
        if self.tool_input.is_some() {
            return;
        }
        // Don't switch if user manually clicked a tab within 2 seconds
        if self.last_manual_tab.elapsed().as_secs() < 2 {
            return;
        }
        let selected = self.selected.len();
        let new_tab = if selected >= 2 {
            2 // Combine
        } else if selected == 1 {
            1 // Transform
        } else {
            0 // Create
        };
        self.active_tab = new_tab;
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
            "pattern" => {
                let count = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(3);
                self.pattern_selected(count)?;
            }
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
                    self.set_status(format!("Exported to {}", path.display()));
                } else {
                    self.set_status("Usage: export <path.stl>");
                }
            }
            "undo" => self.undo()?,
            "redo" => self.redo()?,
            "quit" | "q" => {
                self.running = false;
            }
            // -- Menu-bar commands that don't yet have full TUI support --
            // These are routed through `process_command` so the menu, command
            // palette, and Alt accelerators all share one dispatch path.
            "new" => {
                self.document = Document::new();
                self.selected.clear();
                self.undo_stack.clear();
                self.redo_stack.clear();
                self.file_path = None;
                self.dirty = false;
                self.evaluate()?;
                self.set_status("New document");
            }
            "open" => self.set_status("Open: drag a .vcad file into the terminal"),
            "export_glb" => self.set_status("Export GLB: not yet implemented in TUI"),
            "export_step" => self.set_status("Export STEP: not yet implemented in TUI"),
            "select_all" => {
                let ids: Vec<_> = self.get_parts().into_iter().map(|(id, _)| id).collect();
                self.selected = ids.into_iter().collect();
                self.set_status(format!("Selected {} parts", self.selected.len()));
            }
            "deselect" => {
                self.selected.clear();
                self.set_status("Deselected");
            }
            "toggle_sidebar" => {
                self.sidebar_visible = !self.sidebar_visible;
            }
            "toggle_chat" => {
                // Keep `open` and `focused` in sync so closing via the
                // menu doesn't leave an invisible keystroke trap — keys
                // would otherwise keep routing to the hidden sidebar.
                self.chat.open = !self.chat.open;
                self.chat.focused = self.chat.open;
            }
            "toggle_wireframe" => self.set_status("Wireframe: not yet implemented in TUI"),
            "cycle_theme" => {
                let name = crate::ui::theme::toggle();
                self.set_status(format!("Theme: {name}"));
            }
            "camera_iso" => {
                self.camera
                    .set_orbit(45.0, 30.0, 100.0, crate::render::Vec3::new(0.0, 0.0, 0.0));
                self.set_status("Isometric view");
            }
            "camera_top" => {
                self.camera
                    .set_orbit(0.0, 89.0, 100.0, crate::render::Vec3::new(0.0, 0.0, 0.0));
                self.set_status("Top view");
            }
            "camera_front" => {
                self.camera
                    .set_orbit(0.0, 0.0, 100.0, crate::render::Vec3::new(0.0, 0.0, 0.0));
                self.set_status("Front view");
            }
            "camera_right" => {
                self.camera
                    .set_orbit(90.0, 0.0, 100.0, crate::render::Vec3::new(0.0, 0.0, 0.0));
                self.set_status("Right view");
            }
            "camera_fit" => {
                self.camera.zoom_to_fit(80, 40);
                self.set_status("Fit to screen");
            }
            "palette" => {
                self.mode = TuiMode::Command;
                self.command_input.clear();
                self.command_selected_index = 0;
            }
            "sketch" => {
                self.mode = TuiMode::Sketch(Box::new(crate::tui::SketchModeState::new(
                    crate::tui::SketchPlane::XY,
                )));
                self.set_status("Sketch mode (XY plane) - L:line R:rect C:circle");
            }
            "about" => self.set_status("vcad — parametric CAD for humans and AIs"),
            "open_docs" => self.set_status("Docs: https://vcad.io/docs"),
            "open_github" => self.set_status("GitHub: https://github.com/vcad"),
            "open_discord" => self.set_status("Discord: https://discord.gg/vcad"),
            _ => {
                self.set_status(format!("Unknown command: {}", parts[0]));
            }
        }

        Ok(())
    }
}

/// Evaluate a document to meshes using the canonical vcad-eval evaluator.
pub fn evaluate_document(doc: &Document) -> Result<Vec<EvaluatedMesh>> {
    let opts = vcad_eval::EvalOptions {
        skip_clash_detection: true,
        ..Default::default()
    };
    let scene = vcad_eval::evaluate_document(doc, &opts).map_err(|e| anyhow::anyhow!("{}", e))?;

    Ok(scene
        .parts
        .into_iter()
        .map(|p| EvaluatedMesh {
            vertices: p.mesh.positions,
            indices: p.mesh.indices,
        })
        .collect())
}

/// `vcad_eval::Clock` impl backed by `std::time::Instant`. Lives in the CLI
/// rather than `vcad-eval` so that crate stays free of `std::time` to keep
/// its WASM build clean.
struct NativeClock {
    start: Instant,
}

impl vcad_eval::Clock for NativeClock {
    fn now_ms(&self) -> f64 {
        self.start.elapsed().as_secs_f64() * 1000.0
    }
}

/// Evaluate with timing instrumentation. Mirrors the per-node + per-phase
/// breakdown the browser worker already collects, but headless.
pub fn evaluate_document_timed(
    doc: &Document,
    skip_clash_detection: bool,
) -> Result<(Vec<EvaluatedMesh>, vcad_eval::EvalTiming)> {
    let clock: Box<dyn vcad_eval::Clock> = Box::new(NativeClock {
        start: Instant::now(),
    });
    let opts = vcad_eval::EvalOptions {
        skip_clash_detection,
        clock: Some(clock),
    };
    let scene = vcad_eval::evaluate_document(doc, &opts).map_err(|e| anyhow::anyhow!("{}", e))?;
    let timing = scene
        .timing
        .clone()
        .ok_or_else(|| anyhow::anyhow!("eval did not return timing data"))?;
    let meshes = scene
        .parts
        .into_iter()
        .map(|p| EvaluatedMesh {
            vertices: p.mesh.positions,
            indices: p.mesh.indices,
        })
        .collect();
    Ok((meshes, timing))
}

/// Run the TUI application.
pub fn run_tui(file: Option<PathBuf>) -> Result<()> {
    crate::ui::theme::init();

    // Install panic + stderr capture BEFORE entering alt-screen. Any early
    // stderr writes (e.g. from a failing terminal capability probe) end up
    // in the log store instead of corrupting the cell buffer.
    let capture = crate::log_capture::Capture::install();

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        cursor::Hide
    )?;

    let mut app = App::new(file)?;

    let result = run_loop(&mut stdout, &mut app, &capture);

    disable_raw_mode()?;
    execute!(
        stdout,
        LeaveAlternateScreen,
        DisableMouseCapture,
        cursor::Show
    )?;

    // `capture` drops here — fd 2 is restored before we return so any
    // error printed by the caller goes to the real tty.
    drop(capture);

    result
}

fn run_loop(
    stdout: &mut Stdout,
    app: &mut App,
    capture: &crate::log_capture::Capture,
) -> Result<()> {
    let (term_w, term_h) = terminal::size()?;
    let mut cell_buffer = CellBuffer::new(term_w, term_h);
    let mut render_buffer = RenderBuffer::new(80, 40);
    let mut last_camera = app.camera.snapshot();
    let mut gfx = GraphicsOutput::new();
    let protocol = gfx.protocol();
    let proto_name = match protocol {
        GraphicsProtocol::Kitty => "kitty",
        GraphicsProtocol::ITerm2 => "iterm2",
        GraphicsProtocol::Sixel => "sixel",
        GraphicsProtocol::HalfBlock => "halfblock",
        GraphicsProtocol::Braille => "braille",
    };
    app.set_status(format!("Ready [{proto_name}] +/-:zoom :cmd q:quit"));

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
            GraphicsProtocol::HalfBlock => (area.width as u32, (area.height as u32) * 2),
            GraphicsProtocol::Braille => ((area.width as u32) * 2, (area.height as u32) * 4),
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
                let bg = crate::ui::theme::BG_RGB();
                render_buffer.clear(bg.0, bg.1, bg.2);
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

        // Drain captured stderr/panic lines into the log ring buffer so
        // the status bar surfaces them instead of a corrupt display.
        crate::log_capture::drain_captured(app, capture);

        // Drain any chat events that arrived from the background stream
        // thread and apply them to the chat panel / document.
        crate::chat_session::drain_chat_events(app);

        // Handle input with 16ms poll
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if !crate::input::handle_key(app, key)? => {
                    app.running = false;
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
                    crate::input::handle_mouse(app, mouse, area, &render_buffer, cell_dims)?;
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
        let mut cache = std::collections::HashMap::new();
        if let Ok(Some(solid)) =
            vcad_eval::evaluate_node(entry.root, &app.document.nodes, &mut cache)
        {
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
        .try_normalize()
        .unwrap_or(vcad_kernel::vcad_kernel_math::Vec3::y());
    let up_dir = vcad_kernel::vcad_kernel_math::Dir3::new_normalize(up);

    // Identity transforms for all solids
    let transforms: Vec<f64> = solids
        .iter()
        .flat_map(|_| {
            [
                1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_primitives_render() {
        let shapes: Vec<(&str, vcad_kernel::Solid)> = vec![
            ("cube", vcad_kernel::Solid::cube(20.0, 20.0, 20.0)),
            ("cylinder", vcad_kernel::Solid::cylinder(10.0, 20.0, 32)),
            ("sphere", vcad_kernel::Solid::sphere(10.0, 32)),
            ("cone", vcad_kernel::Solid::cone(10.0, 0.0, 20.0, 32)),
        ];

        for (name, solid) in &shapes {
            let mesh = solid.to_mesh(32);
            assert!(!mesh.vertices.is_empty(), "{name} has no vertices");
            assert!(!mesh.indices.is_empty(), "{name} has no indices");

            // Build triangles
            let mut triangles = Vec::new();
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
                triangles.push(Triangle {
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
                    color: [180, 180, 190],
                    pick_id: 1,
                });
            }

            // Render and check object pixels appear
            let mut buffer = RenderBuffer::new(100, 100);
            let camera = Camera::default();
            crate::render::render_scene(&mut buffer, &triangles, &camera);
            let object_pixels = buffer.pick_ids.iter().filter(|&&id| id > 0).count();
            assert!(object_pixels > 0, "{name} rendered zero object pixels");
        }
    }
}
