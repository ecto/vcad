//! Feature tree sidebar — floating overlay.

use std::collections::HashSet;
use vcad_ir::NodeId;

use super::buffer::{set_char, CellBuffer, Color, Rect};
use super::theme;

/// Draw the floating parts sidebar.
pub fn draw_sidebar(
    buf: &mut CellBuffer,
    parts: &[(NodeId, String)],
    selected: &HashSet<NodeId>,
    focused_index: usize,
    scroll: usize,
    mouse_row: Option<u16>,
    area: Rect,
) {
    let sidebar_width = 22u16;
    let max_visible = parts.len().min(20);
    let sidebar_height = (max_visible + 2) as u16;
    let sidebar_height = sidebar_height.max(4).min(area.height.saturating_sub(6));

    let rect = Rect::new(
        area.x + 1,
        area.y + 2,
        sidebar_width.min(area.width.saturating_sub(2)),
        sidebar_height,
    );

    render_sidebar(buf, rect, parts, selected, focused_index, scroll, mouse_row);
}

fn render_sidebar(
    buf: &mut CellBuffer,
    area: Rect,
    parts: &[(NodeId, String)],
    selected: &HashSet<NodeId>,
    focused_index: usize,
    scroll: usize,
    mouse_row: Option<u16>,
) {
    if area.height < 3 || area.width < 6 {
        return;
    }

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_char(buf, x, y, ' ', theme::SURFACE(), theme::SURFACE());
        }
    }

    // Draw thin box border
    let top = area.y;
    let bot = area.y + area.height - 1;
    let left = area.x;
    let right = area.x + area.width - 1;

    set_char(buf, left, top, '\u{250C}', theme::BORDER(), theme::SURFACE());
    set_char(buf, right, top, '\u{2510}', theme::BORDER(), theme::SURFACE());
    set_char(buf, left, bot, '\u{2514}', theme::BORDER(), theme::SURFACE());
    set_char(buf, right, bot, '\u{2518}', theme::BORDER(), theme::SURFACE());

    for x in (left + 1)..right {
        set_char(buf, x, top, '\u{2500}', theme::BORDER(), theme::SURFACE());
        set_char(buf, x, bot, '\u{2500}', theme::BORDER(), theme::SURFACE());
    }
    for y in (top + 1)..bot {
        set_char(buf, left, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
        set_char(buf, right, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
    }

    // Header: "─ PARTS ─"
    let header = " PARTS ";
    let hx = left + 1;
    set_char(buf, hx, top, '\u{2500}', theme::BORDER(), theme::SURFACE());
    for (i, ch) in header.chars().enumerate() {
        let x = hx + 1 + i as u16;
        if x < right {
            set_char(buf, x, top, ch, theme::TEXT_MUTED(), theme::SURFACE());
        }
    }

    // Parts list
    let inner_top = top + 1;
    let inner_height = (bot - inner_top) as usize;
    let inner_width = (right - left - 1) as usize;

    if parts.is_empty() {
        let msg = "No parts";
        for (i, ch) in msg.chars().enumerate() {
            let x = left + 2 + i as u16;
            if x < right {
                set_char(buf, x, inner_top, ch, theme::TEXT_MUTED(), theme::SURFACE());
            }
        }
        return;
    }

    for (vis_idx, part_idx) in (scroll..).enumerate() {
        if vis_idx >= inner_height || part_idx >= parts.len() {
            break;
        }

        let (id, name) = &parts[part_idx];
        let y = inner_top + vis_idx as u16;
        let is_selected = selected.contains(id);
        let is_focused = part_idx == focused_index;
        let is_hovered = mouse_row.is_some_and(|mr| mr == y);

        let row_bg = if is_selected {
            theme::SELECTION_BG()
        } else if is_hovered || is_focused {
            theme::CARD()
        } else {
            theme::SURFACE()
        };

        // Clear row
        for x in (left + 1)..right {
            set_char(buf, x, y, ' ', row_bg, row_bg);
        }

        // Expand caret
        let caret = if is_selected { '\u{25B8}' } else { ' ' };
        set_char(buf, left + 1, y, ' ', theme::TEXT_MUTED(), row_bg);
        set_char(buf, left + 2, y, caret, theme::TEXT_MUTED(), row_bg);

        // Part icon
        let (icon, icon_color) = part_icon(name);
        set_char(buf, left + 4, y, icon, icon_color, row_bg);

        // Part name
        let name_color = if is_selected {
            theme::ACCENT()
        } else {
            theme::TEXT()
        };
        let max_name_len = inner_width.saturating_sub(6);
        for (i, ch) in name.chars().take(max_name_len).enumerate() {
            let x = left + 6 + i as u16;
            if x < right {
                set_char(buf, x, y, ch, name_color, row_bg);
            }
        }
    }
}

/// Get icon and color for a part based on its name.
fn part_icon(name: &str) -> (char, Color) {
    let lower = name.to_lowercase();
    if lower.contains("cube") || lower.contains("box") {
        ('\u{25A0}', theme::TAB_CREATE)
    } else if lower.contains("cylinder") || lower.contains("cyl") {
        ('\u{25CB}', theme::TAB_TRANSFORM)
    } else if lower.contains("sphere") {
        ('\u{25CF}', theme::TAB_COMBINE)
    } else if lower.contains("cone") {
        ('\u{25B2}', theme::TAB_MODIFY)
    } else if lower.contains("union")
        || lower.contains("difference")
        || lower.contains("intersection")
    {
        ('\u{2295}', theme::TAB_MODIFY)
    } else if lower.contains("translate") || lower.contains("rotate") || lower.contains("scale") {
        ('\u{2194}', theme::TAB_TRANSFORM)
    } else {
        ('\u{25C6}', theme::TEXT_MUTED())
    }
}

/// Returns the Rect of the sidebar for hit-testing.
pub fn sidebar_rect(area: Rect, parts_count: usize) -> Rect {
    let sidebar_width = 22u16;
    let max_visible = parts_count.min(20);
    let sidebar_height = (max_visible + 2) as u16;
    let sidebar_height = sidebar_height.max(4).min(area.height.saturating_sub(6));

    Rect::new(
        area.x + 1,
        area.y + 2,
        sidebar_width.min(area.width.saturating_sub(2)),
        sidebar_height,
    )
}

/// Returns the part index at the given row, if any.
pub fn part_at_row(area: Rect, parts_count: usize, scroll: usize, row: u16) -> Option<usize> {
    let rect = sidebar_rect(area, parts_count);
    if row <= rect.y || row >= rect.y + rect.height - 1 {
        return None;
    }
    let vis_idx = (row - rect.y - 1) as usize;
    let part_idx = scroll + vis_idx;
    if part_idx < parts_count {
        Some(part_idx)
    } else {
        None
    }
}
