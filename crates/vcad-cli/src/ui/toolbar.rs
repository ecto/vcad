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

/// Inline parameter input state — replaces the sub-tool row when active.
pub struct ToolInput {
    /// Display label (e.g. "Fillet Radius")
    pub label: &'static str,
    /// Current numeric value
    pub value: f64,
    /// Minimum allowed value
    pub min: f64,
    /// Maximum allowed value
    pub max: f64,
    /// Scrub step size
    pub step: f64,
    /// Unit suffix (e.g. "mm", "\u{00B0}")
    pub unit: &'static str,
    /// True when user is typing a number directly
    pub editing: bool,
    /// Buffer for direct text entry
    pub edit_buf: String,
    /// Command template — `{}` replaced with the value
    pub command_template: String,
    /// Text-only mode (for filenames, no numeric scrub)
    pub text_mode: bool,
    /// Current axis label for multi-axis tools (e.g. "X", "Y", "Z")
    pub axis: Option<&'static str>,
}

impl ToolInput {
    /// Create a numeric scrub input.
    pub fn numeric(
        label: &'static str,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
        unit: &'static str,
        command_template: String,
    ) -> Self {
        Self {
            label,
            value,
            min,
            max,
            step,
            unit,
            editing: false,
            edit_buf: String::new(),
            command_template,
            text_mode: false,
            axis: None,
        }
    }

    /// Create a text input (for filenames).
    pub fn text(label: &'static str, default: &str, command_template: String) -> Self {
        Self {
            label,
            value: 0.0,
            min: 0.0,
            max: 0.0,
            step: 0.0,
            unit: "",
            editing: true,
            edit_buf: default.to_string(),
            command_template,
            text_mode: true,
            axis: None,
        }
    }

    /// Scrub the value by delta steps, clamping to [min, max].
    pub fn scrub(&mut self, delta: i32, fine: bool) {
        let multiplier = if fine { 0.1 } else { 1.0 };
        self.value += delta as f64 * self.step * multiplier;
        self.value = self.value.clamp(self.min, self.max);
    }

    /// Format the final command string.
    pub fn format_command(&self) -> String {
        if self.text_mode {
            self.command_template.replace("{}", &self.edit_buf)
        } else {
            let val = if self.editing {
                self.edit_buf.parse::<f64>().unwrap_or(self.value)
            } else {
                self.value
            };
            self.command_template.replace("{}", &format!("{val}"))
        }
    }
}

/// Get sub-tools for a given tab index. Indices match `theme::TABS`.
pub fn sub_tools(tab: usize) -> &'static [SubTool] {
    match tab {
        // Create
        0 => &[
            SubTool {
                icon: "\u{25A1}",
                label: "Box",
                command: "cube",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{25CB}",
                label: "Cyl",
                command: "cylinder",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{25CF}",
                label: "Sph",
                command: "sphere",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{25B3}",
                label: "Cone",
                command: "cone",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{270E}",
                label: "Sketch",
                command: "sketch",
                shortcut: Some("S"),
                needs_selection: false,
            },
        ],
        // Transform
        1 => &[
            SubTool {
                icon: "\u{2195}",
                label: "Move",
                command: "move",
                shortcut: Some("wasd"),
                needs_selection: true,
            },
            SubTool {
                icon: "\u{21BB}",
                label: "Rotate",
                command: "rotate",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2922}",
                label: "Scale",
                command: "scale",
                shortcut: None,
                needs_selection: true,
            },
        ],
        // Combine
        2 => &[
            SubTool {
                icon: "\u{222A}",
                label: "Union",
                command: "union",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2216}",
                label: "Diff",
                command: "difference",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2229}",
                label: "Isect",
                command: "intersection",
                shortcut: None,
                needs_selection: true,
            },
        ],
        // Modify
        3 => &[
            SubTool {
                icon: "\u{25E0}",
                label: "Fillet",
                command: "fillet",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2B1F}",
                label: "Chamfer",
                command: "chamfer",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{25A3}",
                label: "Shell",
                command: "shell",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2237}",
                label: "Pattern",
                command: "pattern",
                shortcut: None,
                needs_selection: true,
            },
            SubTool {
                icon: "\u{2194}",
                label: "Mirror",
                command: "mirror",
                shortcut: None,
                needs_selection: true,
            },
        ],
        // Assembly — placeholder sub-tools
        4 => &[
            SubTool {
                icon: "\u{2B12}",
                label: "Part",
                command: "__assembly_stub",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{229E}",
                label: "Instance",
                command: "__assembly_stub",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{2699}",
                label: "Joint",
                command: "__assembly_stub",
                shortcut: None,
                needs_selection: false,
            },
        ],
        // Simulate — placeholder sub-tools
        5 => &[
            SubTool {
                icon: "\u{25B6}",
                label: "Play",
                command: "__simulate_stub",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{23F8}",
                label: "Pause",
                command: "__simulate_stub",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{23ED}",
                label: "Step",
                command: "__simulate_stub",
                shortcut: None,
                needs_selection: false,
            },
        ],
        // Export
        6 => &[
            SubTool {
                icon: "\u{2B07}",
                label: "STL",
                command: "export_stl",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{2B07}",
                label: "STEP",
                command: "export_step",
                shortcut: None,
                needs_selection: false,
            },
            SubTool {
                icon: "\u{1F4BE}",
                label: "Save",
                command: "save",
                shortcut: Some("C-s"),
                needs_selection: false,
            },
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
    tool_input: Option<&ToolInput>,
) {
    let tab_widths: Vec<u16> = theme::TABS
        .iter()
        .map(|(icon, label, _)| (icon.len() + 1 + label.len()) as u16)
        .collect();
    let separators = (theme::TABS.len() - 1) as u16;
    let inner_width: u16 = tab_widths.iter().sum::<u16>() + separators * 3 + 2;
    let total_width = inner_width + 2;

    let tools = sub_tools(active_tab);
    let has_sub_row = !tools.is_empty() || tool_input.is_some();
    let total_height = if has_sub_row { 5u16 } else { 3u16 };

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
        tool_input,
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
    tool_input: Option<&ToolInput>,
) {
    if area.height < 3 || area.width < 10 {
        return;
    }

    let has_sub_row = !tools.is_empty() || tool_input.is_some();

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_char(buf, x, y, ' ', theme::SURFACE(), theme::SURFACE());
        }
    }

    // Draw rounded border
    let top = area.y;
    let bot = area.y + area.height - 1;
    let left = area.x;
    let right = area.x + area.width - 1;

    set_border_char(buf, left, top, '\u{250C}');
    set_border_char(buf, right, top, '\u{2510}');
    set_border_char(buf, left, bot, '\u{2514}');
    set_border_char(buf, right, bot, '\u{2518}');

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
            set_char(buf, cx, tab_row, ' ', theme::SURFACE(), theme::SURFACE());
            cx += 1;
            set_char(
                buf,
                cx,
                tab_row,
                '\u{2502}',
                theme::BORDER(),
                theme::SURFACE(),
            );
            cx += 1;
            set_char(buf, cx, tab_row, ' ', theme::SURFACE(), theme::SURFACE());
            cx += 1;
        }

        let is_active = i == active_tab;
        let is_hovered = mouse_col.is_some_and(|mc| mc >= cx && mc < cx + tab_widths[i])
            && mouse_row.is_some_and(|mr| mr == tab_row);

        let icon_color = *color;
        for ch in icon.chars() {
            if cx < area.x + area.width - 1 {
                set_char(buf, cx, tab_row, ch, icon_color, theme::SURFACE());
                cx += 1;
            }
        }

        if cx < area.x + area.width - 1 {
            set_char(buf, cx, tab_row, ' ', theme::SURFACE(), theme::SURFACE());
            cx += 1;
        }

        let label_color = if is_active {
            *color
        } else if is_hovered {
            theme::TEXT()
        } else {
            theme::TEXT_MUTED()
        };
        for ch in label.chars() {
            if cx < area.x + area.width - 1 {
                set_char(buf, cx, tab_row, ch, label_color, theme::SURFACE());
                cx += 1;
            }
        }
    }

    // Sub-tool row / tool input
    if has_sub_row {
        // Separator line between tabs and sub-tools
        let sep_y = area.y + 2;
        set_char(
            buf,
            left,
            sep_y,
            '\u{251C}',
            theme::BORDER(),
            theme::SURFACE(),
        );
        set_char(
            buf,
            right,
            sep_y,
            '\u{2524}',
            theme::BORDER(),
            theme::SURFACE(),
        );
        for x in (left + 1)..right {
            set_char(buf, x, sep_y, '\u{2500}', theme::BORDER(), theme::SURFACE());
        }

        let sub_row = area.y + 3;

        // If tool_input is active, render the parameter input instead of sub-tool buttons
        if let Some(ti) = tool_input {
            render_tool_input(buf, ti, area, sub_row, bot, active_tab);
        } else {
            render_sub_tools(
                buf,
                tools,
                area,
                sub_row,
                bot,
                active_tab,
                mouse_col,
                mouse_row,
                selected_count,
            );
        }
    }
}

/// Render the inline parameter input on the sub-tool row.
fn render_tool_input(
    buf: &mut CellBuffer,
    ti: &ToolInput,
    area: Rect,
    sub_row: u16,
    bot: u16,
    active_tab: usize,
) {
    let tab_color = theme::tab_color(active_tab);
    let left = area.x;
    let right = area.x + area.width - 1;
    let inner_w = (right - left - 1) as usize;

    // Build display string
    let display = if ti.text_mode {
        let cursor = if ti.editing { "\u{2588}" } else { "" };
        format!("  {}: [{}{}]  ", ti.label, ti.edit_buf, cursor)
    } else if ti.editing {
        let cursor = "\u{2588}";
        let axis_str = ti.axis.map_or(String::new(), |a| format!(" {a}"));
        format!(
            "  {}{}: [{}{}] {}  ",
            ti.label, axis_str, ti.edit_buf, cursor, ti.unit
        )
    } else {
        let axis_str = ti.axis.map_or(String::new(), |a| format!(" {a}"));
        format!(
            "  {}{}: [{:.2}] {}  ",
            ti.label, axis_str, ti.value, ti.unit
        )
    };

    // Center the display string
    let disp_len = display.chars().count();
    let pad = inner_w.saturating_sub(disp_len) / 2;
    let start_x = left + 1 + pad as u16;

    let mut tx = start_x;
    let mut in_label = true;
    for ch in display.chars() {
        if tx >= right {
            break;
        }
        // Color: label part in tab_color, value part in TEXT
        let fg = if ch == '[' {
            in_label = false;
            theme::TEXT()
        } else if ch == ']' {
            in_label = true;
            theme::TEXT()
        } else if in_label {
            tab_color
        } else {
            theme::TEXT()
        };
        set_char(buf, tx, sub_row, ch, fg, theme::SURFACE());
        tx += 1;
    }

    // Hints in bottom border
    let hints = if ti.text_mode {
        "Enter:apply  Esc:cancel"
    } else {
        "\u{2190}\u{2192}:scrub  Enter:apply  Esc:cancel"
    };
    let hint_x = area.x + (area.width.saturating_sub(hints.len() as u16)) / 2;
    set_string(
        buf,
        hint_x,
        bot,
        hints,
        theme::TEXT_MUTED(),
        theme::SURFACE(),
    );
}

/// Render the normal sub-tool buttons.
#[allow(clippy::too_many_arguments)]
fn render_sub_tools(
    buf: &mut CellBuffer,
    tools: &[SubTool],
    area: Rect,
    sub_row: u16,
    bot: u16,
    active_tab: usize,
    mouse_col: Option<u16>,
    mouse_row: Option<u16>,
    selected_count: usize,
) {
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
    let tools_total: u16 =
        tool_widths.iter().sum::<u16>() + (tools.len().saturating_sub(1) as u16) * 3; // 3 chars between tools
    let tools_start = area.x + (area.width.saturating_sub(tools_total)) / 2;

    let mut tx = tools_start;
    for (i, tool) in tools.iter().enumerate() {
        if i > 0 {
            set_char(buf, tx, sub_row, ' ', theme::SURFACE(), theme::SURFACE());
            tx += 1;
            set_char(
                buf,
                tx,
                sub_row,
                '\u{00B7}',
                theme::BORDER(),
                theme::SURFACE(),
            );
            tx += 1;
            set_char(buf, tx, sub_row, ' ', theme::SURFACE(), theme::SURFACE());
            tx += 1;
        }

        let enabled = !tool.needs_selection || selected_count > 0;
        let is_hovered = mouse_col.is_some_and(|mc| mc >= tx && mc < tx + tool_widths[i])
            && mouse_row.is_some_and(|mr| mr == sub_row);

        let fg = if !enabled {
            disabled_color
        } else if is_hovered {
            theme::TEXT()
        } else {
            tab_color
        };

        // Icon
        for ch in tool.icon.chars() {
            if tx < area.x + area.width - 1 {
                set_char(buf, tx, sub_row, ch, fg, theme::SURFACE());
                tx += 1;
            }
        }
        // Space
        if tx < area.x + area.width - 1 {
            set_char(buf, tx, sub_row, ' ', theme::SURFACE(), theme::SURFACE());
            tx += 1;
        }
        // Label
        let label_fg = if !enabled {
            disabled_color
        } else if is_hovered {
            theme::TEXT()
        } else {
            theme::TEXT_MUTED()
        };
        for ch in tool.label.chars() {
            if tx < area.x + area.width - 1 {
                set_char(buf, tx, sub_row, ch, label_fg, theme::SURFACE());
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
        set_string(
            buf,
            hint_x,
            bot,
            &hints,
            theme::TEXT_MUTED(),
            theme::SURFACE(),
        );
    }
}

fn set_border_char(buf: &mut CellBuffer, x: u16, y: u16, ch: char) {
    set_char(buf, x, y, ch, theme::BORDER(), theme::SURFACE());
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
