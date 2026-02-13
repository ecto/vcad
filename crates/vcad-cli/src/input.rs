//! Input handling — mouse + keyboard event dispatch, hit-testing, drag state.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::ui::buffer::Rect;

use crate::app::App;
use crate::tui::TuiMode;

/// Active drag operation.
#[derive(Debug, Clone, Default)]
pub enum DragState {
    #[default]
    None,
    Orbit { start_x: u16, start_y: u16 },
    Pan { start_x: u16, start_y: u16 },
}

/// Tracks double-click detection.
#[derive(Debug, Clone)]
pub struct ClickTracker {
    last_time: Instant,
    last_pos: (u16, u16),
}

impl Default for ClickTracker {
    fn default() -> Self {
        Self {
            last_time: Instant::now(),
            last_pos: (0, 0),
        }
    }
}

impl ClickTracker {
    /// Record a click and return true if it's a double-click.
    pub fn click(&mut self, col: u16, row: u16) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_time);
        let same_pos = (self.last_pos.0 as i16 - col as i16).unsigned_abs() <= 1
            && (self.last_pos.1 as i16 - row as i16).unsigned_abs() <= 1;
        let is_double = elapsed.as_millis() < 300 && same_pos;
        self.last_time = now;
        self.last_pos = (col, row);
        is_double
    }
}

/// Which UI region was hit.
#[derive(Debug, Clone, PartialEq)]
pub enum HitRegion {
    Viewport,
    TopBar,
    SidebarToggle,
    Sidebar(usize),     // part index
    Toolbar(usize),     // tab index
    SubTool(usize),     // sub-tool index within active tab
    CommandPalette(usize), // item index
    StatusBar,
}

/// Determine which UI region a mouse position falls in.
pub fn hit_test(app: &App, area: Rect, col: u16, row: u16) -> HitRegion {
    // Top bar: row 0
    if row == area.y {
        // Sidebar toggle at col 1
        if col <= area.x + 2 {
            return HitRegion::SidebarToggle;
        }
        return HitRegion::TopBar;
    }

    // Status bar: last row
    if row == area.y + area.height - 1 {
        return HitRegion::StatusBar;
    }

    // Bottom toolbar
    let toolbar_rect = crate::ui::toolbar::toolbar_rect(area, app.active_tab);
    if row >= toolbar_rect.y
        && row < toolbar_rect.y + toolbar_rect.height
        && col >= toolbar_rect.x
        && col < toolbar_rect.x + toolbar_rect.width
    {
        // Check sub-tools first (more specific)
        if let Some(idx) = crate::ui::toolbar::sub_tool_at(area, app.active_tab, col, row) {
            return HitRegion::SubTool(idx);
        }
        if let Some(tab) = crate::ui::toolbar::tab_at_column(area, col, row) {
            return HitRegion::Toolbar(tab);
        }
    }

    // Command palette (if open)
    if app.command_mode() {
        let items = crate::ui::command::build_command_items(&app.command_input);
        let pal_rect = crate::ui::command::palette_rect(area, items.len());
        if row >= pal_rect.y
            && row < pal_rect.y + pal_rect.height
            && col >= pal_rect.x
            && col < pal_rect.x + pal_rect.width
        {
            if let Some(idx) = crate::ui::command::item_at_row(area, items.len(), row) {
                return HitRegion::CommandPalette(idx);
            }
        }
    }

    // Sidebar
    if app.sidebar_visible {
        let parts = app.get_parts();
        let sb_rect = crate::ui::tree::sidebar_rect(area, parts.len());
        if row >= sb_rect.y
            && row < sb_rect.y + sb_rect.height
            && col >= sb_rect.x
            && col < sb_rect.x + sb_rect.width
        {
            if let Some(idx) =
                crate::ui::tree::part_at_row(area, parts.len(), app.sidebar_scroll, row)
            {
                return HitRegion::Sidebar(idx);
            }
        }
    }

    // Default: viewport
    HitRegion::Viewport
}

/// Handle a mouse event. Returns Ok(true) if the event was consumed.
///
/// `cell_dims` is `Some((cell_width, cell_height))` for pixel protocols,
/// `None` for half-block (default pick_at behavior).
pub fn handle_mouse(
    app: &mut App,
    event: MouseEvent,
    area: Rect,
    render_buffer: &crate::render::RenderBuffer,
    cell_dims: Option<(u32, u32)>,
) -> anyhow::Result<bool> {
    let col = event.column;
    let row = event.row;

    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let region = hit_test(app, area, col, row);
            match region {
                HitRegion::SidebarToggle => {
                    app.sidebar_visible = !app.sidebar_visible;
                }
                HitRegion::Sidebar(part_idx) => {
                    let parts = app.get_parts();
                    if part_idx < parts.len() {
                        let id = parts[part_idx].0;
                        if event.modifiers.contains(KeyModifiers::SHIFT) {
                            if app.selected.contains(&id) {
                                app.selected.remove(&id);
                            } else {
                                app.selected.insert(id);
                            }
                        } else {
                            app.selected.clear();
                            app.selected.insert(id);
                        }
                        app.focused_part_index = part_idx;
                    }
                }
                HitRegion::Toolbar(tab) => {
                    app.active_tab = tab;
                }
                HitRegion::SubTool(tool_idx) => {
                    let tools = crate::ui::toolbar::sub_tools(app.active_tab);
                    if tool_idx < tools.len() {
                        let tool = &tools[tool_idx];
                        if tool.needs_selection && app.selected.is_empty() {
                            app.status = format!("{} requires a selection", tool.label);
                        } else if tool.command == "sketch" {
                            app.mode = TuiMode::Sketch(crate::tui::SketchModeState::new(
                                crate::tui::SketchPlane::XY,
                            ));
                            app.status =
                                "Sketch mode (XY plane) - L:line R:rect C:circle".to_string();
                        } else {
                            let cmd = tool.command.to_string();
                            app.process_command(&cmd)?;
                        }
                    }
                }
                HitRegion::CommandPalette(item_idx) => {
                    app.command_selected_index = item_idx;
                    // Execute the selected command
                    let items = crate::ui::command::build_command_items(&app.command_input);
                    if item_idx < items.len() {
                        let cmd_label = items[item_idx].label.to_lowercase();
                        app.command_input.clear();
                        app.mode = TuiMode::Normal;
                        app.process_command(&cmd_label)?;
                    }
                }
                HitRegion::Viewport => {
                    let is_double = app.click_tracker.click(col, row);
                    if is_double {
                        // Double-click viewport background: reset camera
                        app.camera = crate::render::Camera::default();
                        app.status = "Camera reset".to_string();
                    } else {
                        // Click-to-select via pick buffer
                        let pick_id = if let Some((cw, ch)) = cell_dims {
                            render_buffer.pick_at_for_protocol(col, row, cw, ch)
                        } else {
                            render_buffer.pick_at(col, row)
                        };
                        if pick_id > 0 {
                            let node_id = pick_id as u64;
                            if event.modifiers.contains(KeyModifiers::SHIFT) {
                                if app.selected.contains(&node_id) {
                                    app.selected.remove(&node_id);
                                } else {
                                    app.selected.insert(node_id);
                                }
                            } else {
                                app.selected.clear();
                                app.selected.insert(node_id);
                            }
                            app.status = format!("{} selected", app.selected.len());
                        } else {
                            app.selected.clear();
                        }
                    }
                }
                _ => {}
            }
        }
        MouseEventKind::Down(MouseButton::Right) => {
            app.drag = DragState::Orbit {
                start_x: col,
                start_y: row,
            };
        }
        MouseEventKind::Down(MouseButton::Middle) => {
            app.drag = DragState::Pan {
                start_x: col,
                start_y: row,
            };
        }
        MouseEventKind::Drag(MouseButton::Right) => {
            if let DragState::Orbit { start_x, start_y } = app.drag {
                let dx = (col as f32 - start_x as f32) * 0.5;
                let dy = (row as f32 - start_y as f32) * 0.5;
                app.camera.rotate_horizontal(dx);
                app.camera.rotate_vertical(-dy);
                app.drag = DragState::Orbit {
                    start_x: col,
                    start_y: row,
                };
                app.is_orbiting = true;
            }
        }
        MouseEventKind::Drag(MouseButton::Middle) => {
            if let DragState::Pan { start_x, start_y } = app.drag {
                let dx = col as f32 - start_x as f32;
                let dy = row as f32 - start_y as f32;
                let scale = app.camera.distance * 0.01;
                let az = app.camera.azimuth.to_radians();
                let right_x = az.cos();
                let right_z = -az.sin();
                app.camera.target.x += right_x * dx * scale;
                app.camera.target.z += right_z * dx * scale;
                app.camera.target.y -= dy * scale;
                app.camera.update_position();
                app.drag = DragState::Pan {
                    start_x: col,
                    start_y: row,
                };
                app.is_orbiting = true;
            }
        }
        MouseEventKind::ScrollUp => {
            let region = hit_test(app, area, col, row);
            if let HitRegion::Sidebar(_) = region {
                app.sidebar_scroll = app.sidebar_scroll.saturating_sub(1);
            } else {
                app.camera.zoom(0.9);
            }
        }
        MouseEventKind::ScrollDown => {
            let region = hit_test(app, area, col, row);
            if let HitRegion::Sidebar(_) = region {
                let parts_count = app.get_parts().len();
                app.sidebar_scroll = (app.sidebar_scroll + 1).min(parts_count.saturating_sub(1));
            } else {
                app.camera.zoom(1.1);
            }
        }
        MouseEventKind::Up(_) => {
            app.is_orbiting = false;
            app.drag = DragState::None;
        }
        MouseEventKind::Moved => {
            app.mouse_pos = (col, row);
        }
        _ => {}
    }

    Ok(true)
}

/// Handle a key event. Returns Ok(true) if the app should continue running.
pub fn handle_key(app: &mut App, key: KeyEvent) -> anyhow::Result<bool> {
    match &mut app.mode {
        TuiMode::Command => match key.code {
            KeyCode::Enter => {
                let items = crate::ui::command::build_command_items(&app.command_input);
                if !items.is_empty() && app.command_selected_index < items.len() {
                    let cmd = items[app.command_selected_index].label.to_lowercase();
                    app.command_input.clear();
                    app.command_selected_index = 0;
                    app.mode = TuiMode::Normal;
                    app.process_command(&cmd)?;
                } else {
                    let cmd = app.command_input.clone();
                    app.command_input.clear();
                    app.command_selected_index = 0;
                    app.mode = TuiMode::Normal;
                    if !cmd.is_empty() {
                        app.process_command(&cmd)?;
                    }
                }
            }
            KeyCode::Esc => {
                app.command_input.clear();
                app.command_selected_index = 0;
                app.mode = TuiMode::Normal;
            }
            KeyCode::Backspace => {
                app.command_input.pop();
                app.command_selected_index = 0;
            }
            KeyCode::Up => {
                if app.command_selected_index > 0 {
                    app.command_selected_index -= 1;
                }
            }
            KeyCode::Down => {
                let items = crate::ui::command::build_command_items(&app.command_input);
                if app.command_selected_index + 1 < items.len() {
                    app.command_selected_index += 1;
                }
            }
            KeyCode::Char(c) => {
                app.command_input.push(c);
                app.command_selected_index = 0;
            }
            _ => {}
        },
        TuiMode::Sketch(state) => match key.code {
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
        },
        TuiMode::Normal => match key.code {
            KeyCode::Char('q') => {
                return Ok(false); // signal quit
            }
            KeyCode::Char(':') | KeyCode::Char('/') => {
                app.mode = TuiMode::Command;
            }
            KeyCode::Char('`') => {
                app.sidebar_visible = !app.sidebar_visible;
            }
            KeyCode::Char('S') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.mode = TuiMode::Sketch(crate::tui::SketchModeState::new(
                    crate::tui::SketchPlane::XY,
                ));
                app.status = "Sketch mode (XY plane) - L:line R:rect C:circle".to_string();
            }
            // Tab switching (1-8 for toolbar tabs — only in Normal mode without primitives)
            // Keeping 1-3 for primitives for backward compat
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
            KeyCode::Char('4') => app.active_tab = 3,
            KeyCode::Char('5') => app.active_tab = 4,
            KeyCode::Char('6') => app.active_tab = 5,
            KeyCode::Char('7') => app.active_tab = 6,
            KeyCode::Char('8') => app.active_tab = 7,
            KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                app.delete_selected()?;
            }
            KeyCode::Char('u') => {
                app.undo()?;
            }
            KeyCode::Char('r') if !key.modifiers.contains(KeyModifiers::CONTROL)
                && !key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                app.redo()?;
            }
            KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.raytrace_enabled = !app.raytrace_enabled;
                app.render_dirty = true;
                app.status = if app.raytrace_enabled {
                    "Ray tracing ON".to_string()
                } else {
                    "Ray tracing OFF".to_string()
                };
            }
            KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.save()?;
            }
            // Camera keyboard controls
            KeyCode::Left => app.camera.rotate_horizontal(-15.0),
            KeyCode::Right => app.camera.rotate_horizontal(15.0),
            KeyCode::Up => app.camera.rotate_vertical(15.0),
            KeyCode::Down => app.camera.rotate_vertical(-15.0),
            KeyCode::Char('+') | KeyCode::Char('=') => app.camera.zoom(0.8),
            KeyCode::Char('-') => app.camera.zoom(1.25),
            // Part selection
            KeyCode::Tab => {
                let parts = app.get_parts();
                if !parts.is_empty() {
                    app.focused_part_index = (app.focused_part_index + 1) % parts.len();
                    app.selected.clear();
                    app.selected.insert(parts[app.focused_part_index].0);
                }
            }
            KeyCode::Esc => {
                app.selected.clear();
            }
            KeyCode::Enter => {
                let parts = app.get_parts();
                if app.focused_part_index < parts.len() {
                    let id = parts[app.focused_part_index].0;
                    if app.selected.contains(&id) {
                        app.selected.remove(&id);
                    } else {
                        app.selected.insert(id);
                    }
                }
            }
            // WASD translation
            KeyCode::Char('w') => app.translate_selected(0.0, 0.0, 5.0)?,
            KeyCode::Char('s') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                app.translate_selected(0.0, 0.0, -5.0)?
            }
            KeyCode::Char('a') => app.translate_selected(-5.0, 0.0, 0.0)?,
            KeyCode::Char('d') => app.translate_selected(5.0, 0.0, 0.0)?,
            _ => {}
        },
        // Other modes — Esc to exit
        _ => {
            if key.code == KeyCode::Esc {
                app.mode = TuiMode::Normal;
                app.status = "Ready".to_string();
            }
        }
    }

    Ok(true)
}
