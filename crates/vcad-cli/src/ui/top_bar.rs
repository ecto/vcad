//! Top bar — Borland-style menu bar with right-cluster affordances.
//!
//! Delegates the menu labels to `ui::menu::draw_menu_bar`. Adds a modified-dot
//! next to the logo when the document is dirty, and a right-aligned cluster
//! that mirrors `Header.tsx` (dirty indicator, mode name). The search chip +
//! bell + user menu from the web header are deferred to M2/M3 — they need
//! state that the TUI doesn't track yet.

use super::buffer::{set_char, set_string, CellBuffer, Rect};
use super::menu;
use super::theme;

use crate::app::App;

/// Draw the top bar across row 0 of `area`.
pub fn draw_top_bar(buf: &mut CellBuffer, app: &App, area: Rect) {
    menu::draw_menu_bar(buf, area, &app.menu_state);
    draw_right_cluster(buf, app, area);
}

/// Right-aligned cluster: dirty dot + mode name.
fn draw_right_cluster(buf: &mut CellBuffer, app: &App, area: Rect) {
    let y = area.y;
    let mode = app.mode.name();
    let dirty = app.is_dirty();

    // Build the right-hand text: optional "● " + mode name + trailing space.
    let dot_w = if dirty { 2 } else { 0 };
    let mode_w = mode.chars().count() as u16;
    let total_w = dot_w + mode_w + 1;

    if area.width <= total_w + 2 {
        return;
    }

    let start = area.x + area.width - total_w - 1;
    let mut x = start;

    if dirty {
        set_char(buf, x, y, '\u{25CF}', theme::ACCENT(), theme::SURFACE()); // ●
        x += 1;
        set_char(buf, x, y, ' ', theme::TEXT(), theme::SURFACE());
        x += 1;
    }

    let mode_color = match mode {
        "NORMAL" => theme::GREEN(),
        "COMMAND" => theme::ACCENT(),
        _ => theme::CYAN(),
    };
    set_string(buf, x, y, mode, mode_color, theme::SURFACE());
}
