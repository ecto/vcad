//! Input handling — mouse + keyboard event dispatch, hit-testing, drag state.

use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};

use crate::ui::buffer::Rect;
use crate::ui::toolbar::ToolInput;

use crate::app::App;
use crate::tui::TuiMode;

/// Active drag operation.
#[derive(Debug, Clone, Default)]
pub enum DragState {
    #[default]
    None,
    Orbit {
        start_x: u16,
        start_y: u16,
    },
    Pan {
        start_x: u16,
        start_y: u16,
    },
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
    Sidebar(usize),        // part index
    Toolbar(usize),        // tab index
    SubTool(usize),        // sub-tool index within active tab
    CommandPalette(usize), // item index
    Terminal,
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

    // Terminal panel (when open, consumes clicks in its region)
    if app.chat.open {
        let tr = crate::ui::chat::chat_rect(area);
        if row >= tr.y && row < tr.y + tr.height && col >= tr.x && col < tr.x + tr.width {
            return HitRegion::Terminal;
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

/// Intercept left-clicks related to the menu bar.
///
/// Returns `true` if the click was consumed by menu-bar logic (label click,
/// popover item click, or dismiss). Returns `false` if the caller should
/// continue with normal hit-testing.
fn handle_menu_click(app: &mut App, area: Rect, col: u16, row: u16) -> anyhow::Result<bool> {
    use crate::ui::menu;

    // Case 1: a menu is currently open — intercept to run an item, switch
    // menus, or dismiss on outside-click.
    if let Some(open_idx) = app.menu_state.open {
        // Item inside the open dropdown?
        if let Some(item_idx) = menu::item_at(area, open_idx, col, row) {
            if let Some(cmd) = menu::item_command(open_idx, item_idx) {
                app.menu_state.close();
                app.process_command(cmd)?;
            }
            return Ok(true);
        }
        // Click on a different menu-bar label switches menus instead of
        // closing.
        if let Some(new_idx) = menu::menu_at(area, col, row) {
            app.menu_state.open_menu(new_idx);
            return Ok(true);
        }
        // Click on the same label closes the menu.
        let open_rect = menu::menu_label_rect(area, open_idx);
        if row == open_rect.y && col >= open_rect.x && col < open_rect.x + open_rect.width {
            app.menu_state.close();
            return Ok(true);
        }
        // Anywhere else — dismiss without running anything.
        app.menu_state.close();
        return Ok(true);
    }

    // Case 2: no menu open — opening a menu on label click.
    if let Some(idx) = menu::menu_at(area, col, row) {
        app.menu_state.open_menu(idx);
        return Ok(true);
    }

    Ok(false)
}

/// Open the appropriate ToolInput for a sub-tool click, or execute directly.
/// Returns true if the tool was handled (either opened input or executed).
fn handle_sub_tool_click(app: &mut App, tool_idx: usize) -> anyhow::Result<bool> {
    let tools = crate::ui::toolbar::sub_tools(app.active_tab);
    if tool_idx >= tools.len() {
        return Ok(false);
    }

    let tool = &tools[tool_idx];

    // Check selection requirement
    if tool.needs_selection && app.selected.is_empty() {
        app.set_status(format!("{} requires a selection", tool.label));
        return Ok(true);
    }

    match tool.command {
        // Sketch mode — special handling
        "sketch" => {
            app.mode = TuiMode::Sketch(Box::new(crate::tui::SketchModeState::new(
                crate::tui::SketchPlane::XY,
            )));
            app.status = "Sketch mode (XY plane) - L:line R:rect C:circle".to_string();
        }

        // Transform tools — open ToolInput
        "move" => {
            let mut ti = ToolInput::numeric(
                "Move",
                5.0,
                -1000.0,
                1000.0,
                1.0,
                "mm",
                "move {} 0 0".to_string(),
            );
            ti.axis = Some("X");
            app.tool_input = Some(ti);
        }
        "rotate" => {
            let mut ti = ToolInput::numeric(
                "Rotate",
                15.0,
                -360.0,
                360.0,
                15.0,
                "\u{00B0}",
                "rotate 0 {} 0".to_string(),
            );
            ti.axis = Some("Y");
            app.tool_input = Some(ti);
        }
        "scale" => {
            app.tool_input = Some(ToolInput::numeric(
                "Scale",
                2.0,
                0.01,
                100.0,
                0.1,
                "\u{00D7}",
                "scale {}".to_string(),
            ));
        }

        // Modify tools — some need ToolInput, some execute directly
        "fillet" => {
            app.tool_input = Some(ToolInput::numeric(
                "Fillet Radius",
                2.0,
                0.1,
                50.0,
                0.5,
                "mm",
                "fillet {}".to_string(),
            ));
        }
        "chamfer" => {
            app.tool_input = Some(ToolInput::numeric(
                "Chamfer Dist",
                2.0,
                0.1,
                50.0,
                0.5,
                "mm",
                "chamfer {}".to_string(),
            ));
        }
        "shell" => {
            app.tool_input = Some(ToolInput::numeric(
                "Shell Thickness",
                1.0,
                0.1,
                20.0,
                0.5,
                "mm",
                "shell {}".to_string(),
            ));
        }
        "pattern" => {
            app.tool_input = Some(ToolInput::numeric(
                "Pattern Count",
                3.0,
                2.0,
                20.0,
                1.0,
                "",
                "pattern {}".to_string(),
            ));
        }
        "mirror" => {
            // Execute immediately — no params
            app.process_command("mirror")?;
        }

        // Export tools — text input for filename
        "export_stl" => {
            app.tool_input = Some(ToolInput::text(
                "Export STL",
                "output.stl",
                "export {}".to_string(),
            ));
        }
        "export_step" => {
            app.tool_input = Some(ToolInput::text(
                "Export STEP",
                "output.step",
                "export {}".to_string(),
            ));
        }

        // Assembly / Simulate stubs
        "__assembly_stub" => {
            app.set_status("Assembly: not yet available in TUI");
        }
        "__simulate_stub" => {
            app.set_status("Simulation: not yet available in TUI");
        }

        // Everything else: execute the command directly
        _ => {
            let cmd = tool.command.to_string();
            app.process_command(&cmd)?;
        }
    }

    Ok(true)
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
            // Menu-bar click handling comes first — an open dropdown
            // intercepts all clicks so the user can dismiss or re-target it.
            if handle_menu_click(app, area, col, row)? {
                return Ok(true);
            }

            // If tool_input is active, clicking outside cancels it
            if app.tool_input.is_some() {
                let region = hit_test(app, area, col, row);
                match region {
                    HitRegion::SubTool(_) | HitRegion::Toolbar(_) => {
                        // Let toolbar/sub-tool clicks through — cancel input first
                        app.tool_input = None;
                    }
                    _ => {
                        // Click anywhere else cancels
                        app.tool_input = None;
                        return Ok(true);
                    }
                }
            }

            let region = hit_test(app, area, col, row);
            // Focus follows the click: clicking the chat sidebar focuses
            // it, anything else unfocuses.
            app.chat.focused = matches!(region, HitRegion::Terminal);
            match region {
                HitRegion::Terminal => {
                    // Sidebar click — already focused above, nothing else to do.
                }
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
                        app.auto_switch_tab();
                    }
                }
                HitRegion::Toolbar(tab) => {
                    app.active_tab = tab;
                    app.last_manual_tab = Instant::now();
                }
                HitRegion::SubTool(tool_idx) => {
                    handle_sub_tool_click(app, tool_idx)?;
                }
                HitRegion::CommandPalette(item_idx) => {
                    app.command_selected_index = item_idx;
                    // Execute the selected command via its canonical id.
                    let items = crate::ui::command::build_command_items(&app.command_input);
                    if item_idx < items.len() {
                        let cmd_id = items[item_idx].id.clone();
                        app.command_input.clear();
                        app.mode = TuiMode::Normal;
                        app.process_command(&cmd_id)?;
                    }
                }
                HitRegion::Viewport => {
                    let is_double = app.click_tracker.click(col, row);
                    if is_double {
                        // Double-click viewport background: reset camera
                        app.camera = crate::render::Camera::default();
                        app.set_status("Camera reset");
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
                            app.set_status(format!("{} selected", app.selected.len()));
                        } else {
                            app.selected.clear();
                        }
                        app.auto_switch_tab();
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
                app.camera.zoom(0.8);
            }
        }
        MouseEventKind::ScrollDown => {
            let region = hit_test(app, area, col, row);
            if let HitRegion::Sidebar(_) = region {
                let parts_count = app.get_parts().len();
                app.sidebar_scroll = (app.sidebar_scroll + 1).min(parts_count.saturating_sub(1));
            } else {
                app.camera.zoom(1.25);
            }
        }
        MouseEventKind::Up(_) => {
            app.is_orbiting = false;
            app.drag = DragState::None;
        }
        MouseEventKind::Moved => {
            app.mouse_pos = (col, row);
            // Update the status-bar cursor XYZ readout whenever the
            // mouse is over the viewport. The viewport spans the full
            // area; the menu bar (row 0), tool strip (rows 1-2), and
            // status bar (last row) overlay it, so we only raycast
            // when the mouse is actually in the 3D region.
            let top_chrome = 3u16;
            let bot_chrome = 1u16;
            let viewport_h = area.height.saturating_sub(top_chrome + bot_chrome);
            let in_viewport = row >= area.y + top_chrome
                && row < area.y + top_chrome + viewport_h
                && !(app.chat.open && col >= crate::ui::chat::chat_rect(area).x);
            app.cursor_world = if in_viewport {
                crate::raycast::raycast_ground_plane(
                    &app.camera,
                    col,
                    row,
                    area.x,
                    area.y + top_chrome,
                    area.width,
                    viewport_h,
                )
            } else {
                None
            };
        }
        _ => {}
    }

    Ok(true)
}

/// Handle key events for the inline tool input. Returns true if consumed.
fn handle_tool_input_key(app: &mut App, key: KeyEvent) -> anyhow::Result<bool> {
    let ti = match app.tool_input.as_mut() {
        Some(ti) => ti,
        None => return Ok(false),
    };

    match key.code {
        KeyCode::Esc => {
            app.tool_input = None;
            app.set_status("Cancelled");
        }
        KeyCode::Enter => {
            let cmd = ti.format_command();
            app.tool_input = None;
            if !cmd.trim().is_empty() {
                app.process_command(&cmd)?;
            }
        }
        KeyCode::Backspace if ti.editing || ti.text_mode => {
            ti.edit_buf.pop();
        }
        KeyCode::Left => {
            if ti.text_mode {
                // No cursor movement in simple text mode
            } else {
                let fine = key.modifiers.contains(KeyModifiers::SHIFT);
                ti.editing = false;
                ti.scrub(-1, fine);
            }
        }
        KeyCode::Right => {
            if ti.text_mode {
                // No cursor movement in simple text mode
            } else {
                let fine = key.modifiers.contains(KeyModifiers::SHIFT);
                ti.editing = false;
                ti.scrub(1, fine);
            }
        }
        // Axis switching for transform tools (X/Y/Z keys)
        KeyCode::Char('x') | KeyCode::Char('X') if !ti.text_mode && ti.axis.is_some() => {
            ti.axis = Some("X");
            update_axis_template(ti);
        }
        KeyCode::Char('y') | KeyCode::Char('Y') if !ti.text_mode && ti.axis.is_some() => {
            ti.axis = Some("Y");
            update_axis_template(ti);
        }
        KeyCode::Char('z') | KeyCode::Char('Z') if !ti.text_mode && ti.axis.is_some() => {
            ti.axis = Some("Z");
            update_axis_template(ti);
        }
        KeyCode::Char(c) => {
            if ti.text_mode {
                ti.edit_buf.push(c);
            } else if c.is_ascii_digit() || c == '.' || c == '-' {
                // Switch to direct editing mode
                if !ti.editing {
                    ti.editing = true;
                    ti.edit_buf.clear();
                }
                ti.edit_buf.push(c);
            }
        }
        _ => {}
    }

    Ok(true)
}

/// Update the command template based on the current axis for Move/Rotate.
fn update_axis_template(ti: &mut ToolInput) {
    let axis = ti.axis.unwrap_or("X");
    if ti.label == "Move" {
        ti.command_template = match axis {
            "X" => "move {} 0 0".to_string(),
            "Y" => "move 0 {} 0".to_string(),
            "Z" => "move 0 0 {}".to_string(),
            _ => "move {} 0 0".to_string(),
        };
    } else if ti.label == "Rotate" {
        ti.command_template = match axis {
            "X" => "rotate {} 0 0".to_string(),
            "Y" => "rotate 0 {} 0".to_string(),
            "Z" => "rotate 0 0 {}".to_string(),
            _ => "rotate 0 {} 0".to_string(),
        };
    }
}

/// Handle a key event. Returns Ok(true) if the app should continue running.
pub fn handle_key(app: &mut App, key: KeyEvent) -> anyhow::Result<bool> {
    // When the chat input has focus, route keys there first.
    if app.chat.focused {
        match key.code {
            KeyCode::Esc if app.chat_session.is_busy() => {
                // Abort the in-flight request without surrendering focus.
                app.chat_session.abort();
                app.log(
                    crate::app::LogLevel::Info,
                    "chat",
                    "aborted in-flight request",
                );
            }
            KeyCode::Char('`') | KeyCode::Esc => {
                // Actually close the sidebar, not just unfocus. The hint
                // text says "close" and that's what users expect.
                app.chat.open = false;
                app.chat.focused = false;
            }
            KeyCode::Enter => {
                if let Some(msg) = app.chat.send_message() {
                    crate::chat_session::push_user_message(app, msg);
                    if let Err(e) = crate::chat_session::start_chat_turn(app) {
                        app.log(crate::app::LogLevel::Error, "chat", e.to_string());
                    }
                }
            }
            KeyCode::Backspace => {
                app.chat.input.pop();
            }
            KeyCode::Up => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.chat.scroll = app.chat.scroll.saturating_add(1);
                } else {
                    app.chat.history_up();
                }
            }
            KeyCode::Down => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    app.chat.scroll = app.chat.scroll.saturating_sub(1);
                } else {
                    app.chat.history_down();
                }
            }
            KeyCode::PageUp => {
                app.chat.scroll = app.chat.scroll.saturating_add(5);
            }
            KeyCode::PageDown => {
                app.chat.scroll = app.chat.scroll.saturating_sub(5);
            }
            KeyCode::Char(c) => {
                app.chat.input.push(c);
            }
            _ => {}
        }
        return Ok(true);
    }

    // Welcome overlay intercepts all keys when visible
    if app.show_welcome {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') if app.welcome_selected > 0 => {
                app.welcome_selected -= 1;
            }
            KeyCode::Down | KeyCode::Char('j')
                if app.welcome_selected + 1 < crate::ui::welcome::ITEM_COUNT =>
            {
                app.welcome_selected += 1;
            }
            KeyCode::Enter => {
                let action = match app.welcome_selected {
                    0 => crate::ui::welcome::WelcomeAction::Tutorial,
                    1 => crate::ui::welcome::WelcomeAction::BlankProject,
                    2 => crate::ui::welcome::WelcomeAction::OpenFile,
                    _ => crate::ui::welcome::WelcomeAction::Dismiss,
                };
                app.show_welcome = false;
                match action {
                    crate::ui::welcome::WelcomeAction::Tutorial => {
                        // Add a cube as the first tutorial step
                        app.process_command("cube")?;
                        app.set_status("Tutorial: now add a cylinder (open Create tab)");
                    }
                    crate::ui::welcome::WelcomeAction::BlankProject => {
                        app.set_status("Ready — press : for commands, Tab for tools");
                    }
                    crate::ui::welcome::WelcomeAction::OpenFile => {
                        // Enter command mode with "open" pre-filled
                        app.mode = TuiMode::Command;
                        app.command_input = "open".to_string();
                        app.command_selected_index = 0;
                    }
                    crate::ui::welcome::WelcomeAction::Dismiss => {
                        app.set_status("Ready — press : for commands, Tab for tools");
                    }
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                app.show_welcome = false;
                app.set_status("Ready — press : for commands, Tab for tools");
            }
            _ => {}
        }
        return Ok(true);
    }

    // Tool input mode takes priority over normal key handling
    if app.tool_input.is_some() {
        handle_tool_input_key(app, key)?;
        return Ok(true);
    }

    match &mut app.mode {
        TuiMode::Command => match key.code {
            KeyCode::Enter => {
                let items = crate::ui::command::build_command_items(&app.command_input);
                if !items.is_empty() && app.command_selected_index < items.len() {
                    let cmd = items[app.command_selected_index].id.clone();
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
            KeyCode::Up if app.command_selected_index > 0 => {
                app.command_selected_index -= 1;
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
                app.set_status("Exited sketch mode");
            }
            KeyCode::Char(c) if state.handle_key(c) => {
                let name = state.tool_name().to_string();
                app.set_status(format!("Sketch tool: {}", name));
            }
            _ => {}
        },
        TuiMode::Normal => {
            // Try the shared registry first. If it resolves the chord to a
            // command id, dispatch through process_command (the TUI's
            // canonical action map) and skip the legacy match arms below.
            // Anything not in the registry falls through unchanged.
            //
            // Skip dispatch when the menu bar is open — those Esc/arrow/
            // Enter handlers below own the menu interaction.
            if !app.menu_state.is_open() {
                if let Some(chord) = crate::keybinding_adapter::chord_from_crossterm(key) {
                    let mode = crate::keybinding_adapter::app_mode_for(&app.mode);
                    let ctx = crate::keybinding_adapter::when_context_for(app);
                    if let Some(cmd_id) = app.keybindings.resolve(&chord, mode, ctx) {
                        let cmd_id = cmd_id.to_string();
                        // Special case: "quit" returns false to break the
                        // event loop instead of dispatching through
                        // process_command.
                        if cmd_id == "quit" {
                            return Ok(false);
                        }
                        app.process_command(&cmd_id)?;
                        app.auto_switch_tab();
                        return Ok(true);
                    }
                }
            }

            match key.code {
                // Menu bar: Esc closes an open dropdown before anything else.
                KeyCode::Esc if app.menu_state.is_open() => {
                    app.menu_state.close();
                }
                // Alt+F/E/V/T/H opens the corresponding top-level menu.
                KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::ALT) => {
                    if let Some(idx) = crate::ui::menu::accelerator_to_menu(c) {
                        app.menu_state.open_menu(idx);
                    }
                }
                // Arrow keys navigate an open menu; Enter runs the focused item.
                KeyCode::Down if app.menu_state.is_open() => {
                    let open_idx = app.menu_state.open.unwrap();
                    let items = crate::ui::menu::MENUS[open_idx].items;
                    let mut next = app.menu_state.focused_item + 1;
                    while next < items.len()
                        && matches!(items[next], crate::ui::menu::MenuItem::Separator)
                    {
                        next += 1;
                    }
                    if next < items.len() {
                        app.menu_state.focused_item = next;
                    }
                }
                KeyCode::Up if app.menu_state.is_open() => {
                    let open_idx = app.menu_state.open.unwrap();
                    let items = crate::ui::menu::MENUS[open_idx].items;
                    let mut prev = app.menu_state.focused_item;
                    loop {
                        if prev == 0 {
                            break;
                        }
                        prev -= 1;
                        if !matches!(items[prev], crate::ui::menu::MenuItem::Separator) {
                            app.menu_state.focused_item = prev;
                            break;
                        }
                    }
                }
                KeyCode::Left if app.menu_state.is_open() => {
                    let idx = app.menu_state.open.unwrap();
                    let new = if idx == 0 {
                        crate::ui::menu::MENUS.len() - 1
                    } else {
                        idx - 1
                    };
                    app.menu_state.open_menu(new);
                }
                KeyCode::Right if app.menu_state.is_open() => {
                    let idx = app.menu_state.open.unwrap();
                    let new = (idx + 1) % crate::ui::menu::MENUS.len();
                    app.menu_state.open_menu(new);
                }
                KeyCode::Enter if app.menu_state.is_open() => {
                    let open_idx = app.menu_state.open.unwrap();
                    let item_idx = app.menu_state.focused_item;
                    if let Some(cmd) = crate::ui::menu::item_command(open_idx, item_idx) {
                        app.menu_state.close();
                        app.process_command(cmd)?;
                    }
                }
                KeyCode::Char('q') => {
                    return Ok(false); // signal quit
                }
                KeyCode::Char(':') | KeyCode::Char('/') => {
                    app.mode = TuiMode::Command;
                }
                KeyCode::Char('`') => {
                    // Open and focus the chat sidebar.
                    app.chat.open = true;
                    app.chat.focused = true;
                }
                KeyCode::Char('S') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.mode = TuiMode::Sketch(Box::new(crate::tui::SketchModeState::new(
                        crate::tui::SketchPlane::XY,
                    )));
                    app.set_status("Sketch mode (XY plane) - L:line R:rect C:circle");
                }
                // Number keys 1-7: switch toolbar tabs (matching web app)
                KeyCode::Char('1') => {
                    app.active_tab = 0; // Create
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('2') => {
                    app.active_tab = 1; // Transform
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('3') => {
                    app.active_tab = 2; // Combine
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('4') => {
                    app.active_tab = 3; // Modify
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('5') => {
                    app.active_tab = 4; // Assembly
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('6') => {
                    app.active_tab = 5; // Simulate
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('7') => {
                    app.active_tab = 6; // Export
                    app.last_manual_tab = Instant::now();
                }
                KeyCode::Char('x') | KeyCode::Delete | KeyCode::Backspace => {
                    app.delete_selected()?;
                    app.auto_switch_tab();
                }
                KeyCode::Char('u') => {
                    app.undo()?;
                }
                KeyCode::Char('r')
                    if !key.modifiers.contains(KeyModifiers::CONTROL)
                        && !key.modifiers.contains(KeyModifiers::SHIFT) =>
                {
                    app.redo()?;
                }
                KeyCode::Char('R') if key.modifiers.contains(KeyModifiers::SHIFT) => {
                    app.raytrace_enabled = !app.raytrace_enabled;
                    app.render_dirty = true;
                    app.set_status(if app.raytrace_enabled {
                        "Ray tracing ON"
                    } else {
                        "Ray tracing OFF"
                    });
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
                        app.auto_switch_tab();
                    }
                }
                KeyCode::Esc => {
                    app.selected.clear();
                    app.auto_switch_tab();
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
                        app.auto_switch_tab();
                    }
                }
                // WASD translation
                KeyCode::Char('w') => app.translate_selected(0.0, 0.0, 5.0)?,
                KeyCode::Char('s') if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    app.translate_selected(0.0, 0.0, -5.0)?
                }
                KeyCode::Char('a') => app.translate_selected(-5.0, 0.0, 0.0)?,
                KeyCode::Char('d') => app.translate_selected(5.0, 0.0, 0.0)?,
                KeyCode::Char('t') => {
                    let mode_name = crate::ui::theme::toggle();
                    app.render_dirty = true;
                    app.set_status(format!("Theme: {mode_name}"));
                }
                _ => {}
            }
        }
        // Other modes — Esc to exit
        _ => {
            if key.code == KeyCode::Esc {
                app.mode = TuiMode::Normal;
                app.set_status("Ready");
            }
        }
    }

    Ok(true)
}
