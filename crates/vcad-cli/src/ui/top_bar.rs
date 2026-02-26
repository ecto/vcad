//! Top bar overlay — logo, sidebar toggle, and action icons.

use super::buffer::{set_char, set_string, CellBuffer, Rect};
use super::theme;

/// Draw the top bar overlay.
pub fn draw_top_bar(buf: &mut CellBuffer, sidebar_visible: bool, mode_name: &str, area: Rect) {
    let y = area.y;

    // Fill background
    for x in area.x..area.x + area.width {
        set_char(buf, x, y, ' ', theme::SURFACE(), theme::SURFACE());
    }

    // Left side: sidebar toggle + logo
    let sidebar_color = if sidebar_visible {
        theme::ACCENT()
    } else {
        theme::TEXT_MUTED()
    };

    let mut cx = area.x + 1;
    set_char(buf, cx, y, '\u{2630}', sidebar_color, theme::SURFACE()); // ☰
    cx += 3;
    set_string(buf, cx, y, "vcad", theme::TEXT(), theme::SURFACE());
    cx += 4;
    set_char(buf, cx, y, '\u{00B7}', theme::ACCENT(), theme::SURFACE()); // ·

    // Right side: save + settings + mode
    let right_text = format!(" S  \u{2699}  {} ", mode_name);
    let right_width = right_text.len() as u16;

    if area.width > right_width {
        let rx = area.x + area.width - right_width;
        let mut rcx = rx;
        set_char(buf, rcx, y, ' ', theme::SURFACE(), theme::SURFACE());
        rcx += 1;
        set_char(buf, rcx, y, 'S', theme::TEXT_MUTED(), theme::SURFACE());
        rcx += 1;
        set_string(buf, rcx, y, "  ", theme::BORDER(), theme::SURFACE());
        rcx += 2;
        set_char(
            buf,
            rcx,
            y,
            '\u{2699}',
            theme::TEXT_MUTED(),
            theme::SURFACE(),
        );
        rcx += 1;
        set_string(buf, rcx, y, "  ", theme::BORDER(), theme::SURFACE());
        rcx += 2;
        set_string(buf, rcx, y, mode_name, theme::GREEN(), theme::SURFACE());
        rcx += mode_name.len() as u16;
        set_char(buf, rcx, y, ' ', theme::SURFACE(), theme::SURFACE());
    }
}

/// Returns the Rect occupied by the sidebar toggle icon (for click detection).
#[allow(dead_code)]
pub fn sidebar_toggle_rect(area: Rect) -> Rect {
    Rect::new(area.x + 1, area.y, 1, 1)
}

/// Returns the Rect occupied by the save button (for click detection).
#[allow(dead_code)]
pub fn save_button_rect(area: Rect) -> Rect {
    let right_offset = area.width.saturating_sub(10);
    Rect::new(area.x + right_offset, area.y, 1, 1)
}
