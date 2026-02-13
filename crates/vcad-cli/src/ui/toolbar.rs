//! Bottom toolbar — centered pill with tabs and expandable sub-tools.

use super::buffer::{set_char, set_string, CellBuffer, Color, Rect};
use super::theme;

/// A sub-tool within a tab.
pub struct SubTool {
    pub icon: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    pub shortcut: Option<&'static str>,
    pub needs_selection: bool,
}

/// Get sub-tools for a given tab index.
pub fn sub_tools(tab: usize) -> &'static [SubTool] {
    match tab {
        // Chat — no sub-tools
        0 => &[],
        // Create
        1 => &[
            SubTool { icon: "\u{25A1}", label: "Box", command: "cube", shortcut: Some("1"), needs_selection: false },
            SubTool { icon: "\u{25CB}", label: "Cyl", command: "cylinder", shortcut: Some("2"), needs_selection: false },
            SubTool { icon: "\u{25CF}", label: "Sph", command: "sphere", shortcut: Some("3"), needs_selection: false },
            SubTool { icon: "\u{25B3}", label: "Cone", command: "cone", shortcut: None, needs_selection: false },
            SubTool { icon: "\u{270E}", label: "Sketch", command: "sketch", shortcut: Some("S"), needs_selection: false },
        ],
        // Transform
        2 => &[
            SubTool { icon: "\u{2195}", label: "Move", command: "move", shortcut: Some("wasd"), needs_selection: true },
            SubTool { icon: "\u{21BB}", label: "Rotate", command: "rotate", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2922}", label: "Scale", command: "scale", shortcut: None, needs_selection: true },
        ],
        // Combine
        3 => &[
            SubTool { icon: "\u{222A}", label: "Union", command: "union", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2216}", label: "Diff", command: "difference", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2229}", label: "Isect", command: "intersection", shortcut: None, needs_selection: true },
        ],
        // Modify
        4 => &[
            SubTool { icon: "\u{25E0}", label: "Fillet", command: "fillet", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2B1F}", label: "Chamfer", command: "chamfer", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{25A3}", label: "Shell", command: "shell", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2237}", label: "Pattern", command: "pattern", shortcut: None, needs_selection: true },
            SubTool { icon: "\u{2194}", label: "Mirror", command: "mirror", shortcut: None, needs_selection: true },
        ],
        // Assembly — placeholder
        5 => &[],
        // Simulate — placeholder
        6 => &[],
        // Export
        7 => &[
            SubTool { icon: "\u{2B07}", label: "STL", command: "export output.stl", shortcut: None, needs_selection: false },
            SubTool { icon: "\u{2B07}", label: "STEP", command: "export output.step", shortcut: None, needs_selection: false },
            SubTool { icon: "\u{1F4BE}", label: "Save", command: "save", shortcut: Some("C-s"), needs_selection: false },
        ],
        _ => &[],
    }
}

/// Draw the bottom toolbar as a centered pill above the status bar.
pub fn draw_toolbar(
    buf: &mut CellBuffer,
    active_tab: usize,
    area: Rect,
    mouse_col: Option<u16>,
    mouse_row: Option<u16>,
    selected_count: usize,
) {
    let tab_widths: Vec<u16> = theme::TABS
        .iter()
        .map(|(icon, label, _)| (icon.len() + 1 + label.len()) as u16)
        .collect();
    let separators = (theme::TABS.len() - 1) as u16;
    let inner_width: u16 = tab_widths.iter().sum::<u16>() + separators * 3 + 2;
    let total_width = inner_width + 2;

    let tools = sub_tools(active_tab);
    let has_sub_tools = !tools.is_empty();
    let total_height = if has_sub_tools { 5u16 } else { 3u16 };

    let x = area.x + (area.width.saturating_sub(total_width)) / 2;
    let y = area.y + area.height.saturating_sub(total_height + 1);
    let toolbar_rect = Rect::new(x, y, total_width.min(area.width), total_height);

    render_toolbar(
        buf,
        toolbar_rect,
        active_tab,
        mouse_col,
        mouse_row,
        &tab_widths,
        tools,
        selected_count,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_toolbar(
    buf: &mut CellBuffer,
    area: Rect,
    active_tab: usize,
    mouse_col: Option<u16>,
    mouse_row: Option<u16>,
    tab_widths: &[u16],
    tools: &[SubTool],
    selected_count: usize,
) {
    if area.height < 3 || area.width < 10 {
        return;
    }

    let has_sub_tools = !tools.is_empty();

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_char(buf, x, y, ' ', theme::SURFACE, theme::SURFACE);
        }
    }

    // Draw rounded border
    let top = area.y;
    let bot = area.y + area.height - 1;
    let left = area.x;
    let right = area.x + area.width - 1;

    set_border_char(buf, left, top, '\u{256D}');
    set_border_char(buf, right, top, '\u{256E}');
    set_border_char(buf, left, bot, '\u{2570}');
    set_border_char(buf, right, bot, '\u{256F}');

    for x in (left + 1)..right {
        set_border_char(buf, x, top, '\u{2500}');
        set_border_char(buf, x, bot, '\u{2500}');
    }
    for y in (top + 1)..bot {
        set_border_char(buf, left, y, '\u{2502}');
        set_border_char(buf, right, y, '\u{2502}');
    }

    // Draw tabs on the first content row
    let tab_row = area.y + 1;
    let mut cx = area.x + 2;

    for (i, (icon, label, color)) in theme::TABS.iter().enumerate() {
        if i > 0 && cx + 2 < area.x + area.width {
            set_char(buf, cx, tab_row, ' ', theme::SURFACE, theme::SURFACE);
            cx += 1;
            set_char(buf, cx, tab_row, '\u{2502}', theme::BORDER, theme::SURFACE);
            cx += 1;
            set_char(buf, cx, tab_row, ' ', theme::SURFACE, theme::SURFACE);
            cx += 1;
        }

        let is_active = i == active_tab;
        let is_hovered = mouse_col.is_some_and(|mc| mc >= cx && mc < cx + tab_widths[i])
            && mouse_row.is_some_and(|mr| mr == tab_row);

        let icon_color = *color;
        for ch in icon.chars() {
            if cx < area.x + area.width - 1 {
                set_char(buf, cx, tab_row, ch, icon_color, theme::SURFACE);
                cx += 1;
            }
        }

        if cx < area.x + area.width - 1 {
            set_char(buf, cx, tab_row, ' ', theme::SURFACE, theme::SURFACE);
            cx += 1;
        }

        let label_color = if is_active {
            *color
        } else if is_hovered {
            theme::TEXT
        } else {
            theme::TEXT_MUTED
        };
        for ch in label.chars() {
            if cx < area.x + area.width - 1 {
                set_char(buf, cx, tab_row, ch, label_color, theme::SURFACE);
                cx += 1;
            }
        }
    }

    // Sub-tool row
    if has_sub_tools {
        // Separator line between tabs and sub-tools
        let sep_y = area.y + 2;
        set_char(buf, left, sep_y, '\u{251C}', theme::BORDER, theme::SURFACE);
        set_char(buf, right, sep_y, '\u{2524}', theme::BORDER, theme::SURFACE);
        for x in (left + 1)..right {
            set_char(buf, x, sep_y, '\u{2500}', theme::BORDER, theme::SURFACE);
        }

        // Sub-tools on the row below separator
        let sub_row = area.y + 3;
        let tab_color = theme::tab_color(active_tab);
        let disabled_color = Color::rgb(0x55, 0x55, 0x55);

        // Calculate total width of sub-tools for centering
        let tool_widths: Vec<u16> = tools
            .iter()
            .map(|t| {
                let icon_w = t.icon.chars().count() as u16;
                icon_w + 1 + t.label.len() as u16
            })
            .collect();
        let tools_total: u16 = tool_widths.iter().sum::<u16>()
            + (tools.len().saturating_sub(1) as u16) * 3; // 3 chars between tools
        let tools_start = area.x + (area.width.saturating_sub(tools_total)) / 2;

        let mut tx = tools_start;
        for (i, tool) in tools.iter().enumerate() {
            if i > 0 {
                set_char(buf, tx, sub_row, ' ', theme::SURFACE, theme::SURFACE);
                tx += 1;
                set_char(buf, tx, sub_row, '\u{00B7}', theme::BORDER, theme::SURFACE);
                tx += 1;
                set_char(buf, tx, sub_row, ' ', theme::SURFACE, theme::SURFACE);
                tx += 1;
            }

            let enabled = !tool.needs_selection || selected_count > 0;
            let is_hovered = mouse_col.is_some_and(|mc| mc >= tx && mc < tx + tool_widths[i])
                && mouse_row.is_some_and(|mr| mr == sub_row);

            let fg = if !enabled {
                disabled_color
            } else if is_hovered {
                theme::TEXT
            } else {
                tab_color
            };

            // Icon
            for ch in tool.icon.chars() {
                if tx < area.x + area.width - 1 {
                    set_char(buf, tx, sub_row, ch, fg, theme::SURFACE);
                    tx += 1;
                }
            }
            // Space
            if tx < area.x + area.width - 1 {
                set_char(buf, tx, sub_row, ' ', theme::SURFACE, theme::SURFACE);
                tx += 1;
            }
            // Label
            let label_fg = if !enabled {
                disabled_color
            } else if is_hovered {
                theme::TEXT
            } else {
                theme::TEXT_MUTED
            };
            for ch in tool.label.chars() {
                if tx < area.x + area.width - 1 {
                    set_char(buf, tx, sub_row, ch, label_fg, theme::SURFACE);
                    tx += 1;
                }
            }
        }

        // Shortcut hints in the bottom border
        let mut hints = String::new();
        for tool in tools {
            if let Some(sc) = tool.shortcut {
                if !hints.is_empty() {
                    hints.push_str("  ");
                }
                hints.push_str(&format!("{}:{}", sc, tool.label));
            }
        }
        if !hints.is_empty() {
            let hint_x = area.x + (area.width.saturating_sub(hints.len() as u16)) / 2;
            set_string(buf, hint_x, bot, &hints, theme::TEXT_MUTED, theme::SURFACE);
        }
    }
}

fn set_border_char(buf: &mut CellBuffer, x: u16, y: u16, ch: char) {
    set_char(buf, x, y, ch, theme::BORDER, theme::SURFACE);
}

/// Returns the tab index at the given column position, if any.
pub fn tab_at_column(active_area: Rect, col: u16, row: u16) -> Option<usize> {
    let rect = toolbar_rect(active_area, 0);
    let tab_row = rect.y + 1;
    if row != tab_row {
        return None;
    }

    let tab_widths: Vec<u16> = theme::TABS
        .iter()
        .map(|(icon, label, _)| (icon.len() + 1 + label.len()) as u16)
        .collect();
    let separators = (theme::TABS.len() - 1) as u16;
    let inner_width: u16 = tab_widths.iter().sum::<u16>() + separators * 3 + 2;
    let total_width = inner_width + 2;

    let x = active_area.x + (active_area.width.saturating_sub(total_width)) / 2;
    let mut cx = x + 2;

    for (i, tw) in tab_widths.iter().enumerate() {
        if i > 0 {
            cx += 3;
        }
        if col >= cx && col < cx + tw {
            return Some(i);
        }
        cx += tw;
    }
    None
}

/// Returns the sub-tool index at the given column/row, if any.
pub fn sub_tool_at(active_area: Rect, active_tab: usize, col: u16, row: u16) -> Option<usize> {
    let tools = sub_tools(active_tab);
    if tools.is_empty() {
        return None;
    }

    let rect = toolbar_rect(active_area, active_tab);
    let sub_row = rect.y + 3;
    if row != sub_row {
        return None;
    }

    let tool_widths: Vec<u16> = tools
        .iter()
        .map(|t| {
            let icon_w = t.icon.chars().count() as u16;
            icon_w + 1 + t.label.len() as u16
        })
        .collect();
    let tools_total: u16 =
        tool_widths.iter().sum::<u16>() + (tools.len().saturating_sub(1) as u16) * 3;
    let tools_start = rect.x + (rect.width.saturating_sub(tools_total)) / 2;

    let mut tx = tools_start;
    for (i, tw) in tool_widths.iter().enumerate() {
        if i > 0 {
            tx += 3;
        }
        if col >= tx && col < tx + tw {
            return Some(i);
        }
        tx += tw;
    }
    None
}

/// Returns the Rect of the toolbar pill.
pub fn toolbar_rect(area: Rect, active_tab: usize) -> Rect {
    let tab_widths: Vec<u16> = theme::TABS
        .iter()
        .map(|(icon, label, _)| (icon.len() + 1 + label.len()) as u16)
        .collect();
    let separators = (theme::TABS.len() - 1) as u16;
    let inner_width: u16 = tab_widths.iter().sum::<u16>() + separators * 3 + 2;
    let total_width = inner_width + 2;

    let tools = sub_tools(active_tab);
    let total_height = if tools.is_empty() { 3u16 } else { 5u16 };

    let x = area.x + (area.width.saturating_sub(total_width)) / 2;
    let y = area.y + area.height.saturating_sub(total_height + 1);
    Rect::new(x, y, total_width.min(area.width), total_height)
}
