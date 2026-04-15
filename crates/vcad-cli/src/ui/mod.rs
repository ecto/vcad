//! TUI widgets and layout — full-bleed viewport with floating overlays.

pub mod buffer;
pub mod chat;
pub mod command;
pub mod menu;
pub mod status;
pub mod theme;
#[cfg(unix)]
pub mod theme_probe;
pub mod toolbar;
pub mod top_bar;
pub mod tree;
pub mod viewport;

use buffer::{CellBuffer, Rect};

use crate::app::App;
use crate::render::RenderBuffer;

/// Draw the full UI with full-bleed viewport (half-block) and floating overlays.
pub fn draw(buf: &mut CellBuffer, app: &App, render_buffer: &RenderBuffer) {
    let area = Rect::new(0, 0, buf.width, buf.height);

    // Pass 1: full-bleed viewport fills entire terminal
    viewport::render_viewport(buf, render_buffer, area);

    // Pass 2: floating overlays
    draw_overlays_with_area(buf, app, area);
}

/// Draw the full UI with braille viewport and floating overlays.
pub fn draw_braille(buf: &mut CellBuffer, app: &App, render_buffer: &RenderBuffer) {
    let area = Rect::new(0, 0, buf.width, buf.height);

    // Pass 1: braille viewport
    viewport::render_viewport_braille(buf, render_buffer, area);

    // Pass 2: floating overlays
    draw_overlays_with_area(buf, app, area);
}

/// Draw only floating overlays (for pixel protocols where viewport is output directly).
pub fn draw_overlays(buf: &mut CellBuffer, app: &App) {
    let area = Rect::new(0, 0, buf.width, buf.height);
    draw_overlays_with_area(buf, app, area);
}

/// Internal: draw all floating overlay widgets.
fn draw_overlays_with_area(buf: &mut CellBuffer, app: &App, area: Rect) {
    if !app.is_orbiting {
        // Top bar (Borland menu bar + right cluster)
        top_bar::draw_top_bar(buf, app, area);

        // Sidebar (toggleable)
        if app.sidebar_visible {
            let parts = app.get_parts();
            tree::draw_sidebar(
                buf,
                &parts,
                &app.selected,
                app.focused_part_index,
                app.sidebar_scroll,
                Some(app.mouse_pos.1),
                area,
            );
        }

        // Bottom toolbar
        toolbar::draw_toolbar(
            buf,
            app.active_tab,
            area,
            Some(app.mouse_pos.0),
            Some(app.mouse_pos.1),
            app.selected.len(),
            app.tool_input.as_ref(),
        );
    }

    // Status bar — always visible
    status::draw_status_bar(buf, app, area);

    // Open menu dropdown (drawn after toolbar so it overlays it).
    if !app.is_orbiting && app.menu_state.is_open() {
        menu::draw_open_menu(buf, area, &app.menu_state);
    }

    // Command palette (when in command mode)
    if app.command_mode() {
        let items = command::build_command_items(&app.command_input);
        command::draw_command_palette(
            buf,
            &app.command_input,
            &items,
            app.command_selected_index,
            area,
        );
    }

    // Chat panel (when open)
    if app.chat.open {
        chat::draw_chat(buf, &app.chat, area);
    }
}
