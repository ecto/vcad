//! Bottom toolbar — centered pill with tabs and expandable sub-tools.

use super::buffer::{set_char, set_char_underline, set_string, CellBuffer, Rect};
use super::theme;

/// A sub-tool within a tab.
pub struct SubTool {
    pub icon: &'static str,
    pub label: &'static str,
    pub command: &'static str,
    /// Keyboard shortcut hint, surfaced by the status bar in M5.
    #[allow(dead_code)]
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

// ---------------------------------------------------------------------------
// Layout constants — the strip is anchored directly under the menu bar
// (row 0). The tab row is row 1; the optional sub-tools row is row 2.
// ---------------------------------------------------------------------------

/// Row index of the tab strip within `area`. Fixed: directly below menu bar.
pub const TAB_ROW_OFFSET: u16 = 1;
/// Row index of the sub-tools / tool-input strip.
pub const SUB_ROW_OFFSET: u16 = 2;
/// Left pad (in cells) before the first tab / sub-tool.
const LEFT_PAD: u16 = 2;
/// Spaces between adjacent tabs in the strip.
const TAB_GAP: u16 = 3;
/// Spaces between adjacent sub-tools.
const SUB_TOOL_GAP: u16 = 3;

/// Width of a single tab in cells: icon + space + label.
fn tab_width(icon: &str, label: &str) -> u16 {
    (icon.chars().count() + 1 + label.chars().count()) as u16
}

/// Width of a single sub-tool in cells: icon + space + label.
fn sub_tool_width(tool: &SubTool) -> u16 {
    (tool.icon.chars().count() + 1 + tool.label.chars().count()) as u16
}

/// Column where the i-th tab's icon begins.
fn tab_start_x(area: Rect, tab_idx: usize) -> u16 {
    let mut cx = area.x + LEFT_PAD;
    for (i, (icon, label, _)) in theme::TABS.iter().enumerate() {
        if i == tab_idx {
            break;
        }
        cx += tab_width(icon, label) + TAB_GAP;
    }
    cx
}

/// Column where the i-th sub-tool's icon begins (left-aligned strip).
fn sub_tool_start_x(area: Rect, tools: &[SubTool], tool_idx: usize) -> u16 {
    let mut tx = area.x + LEFT_PAD;
    for (i, tool) in tools.iter().enumerate() {
        if i == tool_idx {
            break;
        }
        tx += sub_tool_width(tool) + SUB_TOOL_GAP;
    }
    tx
}

/// Draw the Borland-style tool strip flush under the menu bar. Row 1 is the
/// tab strip; row 2 is the sub-tools / tool-input row, present only when the
/// active tab has sub-tools or a parameter input is active.
pub fn draw_toolbar(
    buf: &mut CellBuffer,
    active_tab: usize,
    area: Rect,
    mouse_col: Option<u16>,
    mouse_row: Option<u16>,
    selected_count: usize,
    tool_input: Option<&ToolInput>,
) {
    if area.width < 10 || area.height < 3 {
        return;
    }

    let tab_row = area.y + TAB_ROW_OFFSET;
    let sub_row = area.y + SUB_ROW_OFFSET;

    let tools = sub_tools(active_tab);
    let has_sub_row = !tools.is_empty() || tool_input.is_some();

    // Fill tab strip background.
    for x in area.x..area.x + area.width {
        set_char(buf, x, tab_row, ' ', theme::TEXT(), theme::SURFACE());
    }

    // Draw tabs left-aligned, separated by TAB_GAP blank cells.
    let mut cx = area.x + LEFT_PAD;
    for (i, (icon, label, color)) in theme::TABS.iter().enumerate() {
        let w = tab_width(icon, label);
        let is_active = i == active_tab;
        let is_hovered = mouse_col.is_some_and(|mc| mc >= cx && mc < cx + w)
            && mouse_row.is_some_and(|mr| mr == tab_row);

        // Icon (always in tab accent color).
        for ch in icon.chars() {
            if cx < area.x + area.width {
                set_char(buf, cx, tab_row, ch, *color, theme::SURFACE());
                cx += 1;
            }
        }
        if cx < area.x + area.width {
            set_char(buf, cx, tab_row, ' ', theme::TEXT(), theme::SURFACE());
            cx += 1;
        }

        // Label (active = underlined in tab color, hover = text, else muted).
        let label_fg = if is_active {
            *color
        } else if is_hovered {
            theme::TEXT()
        } else {
            theme::TEXT_MUTED()
        };
        for ch in label.chars() {
            if cx >= area.x + area.width {
                break;
            }
            if is_active {
                set_char_underline(buf, cx, tab_row, ch, label_fg, theme::SURFACE());
            } else {
                set_char(buf, cx, tab_row, ch, label_fg, theme::SURFACE());
            }
            cx += 1;
        }

        // Blank gap between tabs (not drawn after the last one).
        if i + 1 < theme::TABS.len() {
            cx += TAB_GAP;
        }
    }

    // Sub-tools row / tool_input row.
    if has_sub_row {
        for x in area.x..area.x + area.width {
            set_char(buf, x, sub_row, ' ', theme::TEXT(), theme::SURFACE());
        }
        if let Some(ti) = tool_input {
            render_tool_input_flush(buf, ti, area, sub_row, active_tab);
        } else {
            render_sub_tools_flush(
                buf,
                tools,
                area,
                sub_row,
                active_tab,
                mouse_col,
                mouse_row,
                selected_count,
            );
        }
    }
}

/// Render the inline parameter input on the sub-tool row (left-aligned).
fn render_tool_input_flush(
    buf: &mut CellBuffer,
    ti: &ToolInput,
    area: Rect,
    sub_row: u16,
    active_tab: usize,
) {
    let tab_color = theme::tab_color(active_tab);

    let display = if ti.text_mode {
        let cursor = if ti.editing { "\u{2588}" } else { "" };
        format!("{}: [{}{}]", ti.label, ti.edit_buf, cursor)
    } else if ti.editing {
        let cursor = "\u{2588}";
        let axis_str = ti.axis.map_or(String::new(), |a| format!(" {a}"));
        format!(
            "{}{}: [{}{}] {}",
            ti.label, axis_str, ti.edit_buf, cursor, ti.unit
        )
    } else {
        let axis_str = ti.axis.map_or(String::new(), |a| format!(" {a}"));
        format!("{}{}: [{:.2}] {}", ti.label, axis_str, ti.value, ti.unit)
    };

    let mut tx = area.x + LEFT_PAD;
    let right_edge = area.x + area.width;
    let mut in_label = true;
    for ch in display.chars() {
        if tx >= right_edge {
            break;
        }
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

    // Right-aligned hint.
    let hint = if ti.text_mode {
        "Enter:apply  Esc:cancel"
    } else {
        "\u{2190}\u{2192}:scrub  Enter:apply  Esc:cancel"
    };
    let hint_w = hint.chars().count() as u16;
    if right_edge >= hint_w + LEFT_PAD {
        let hx = right_edge - hint_w - LEFT_PAD;
        if hx > tx + 2 {
            set_string(
                buf,
                hx,
                sub_row,
                hint,
                theme::TEXT_MUTED(),
                theme::SURFACE(),
            );
        }
    }
}

/// Render the left-aligned sub-tool buttons.
#[allow(clippy::too_many_arguments)]
fn render_sub_tools_flush(
    buf: &mut CellBuffer,
    tools: &[SubTool],
    area: Rect,
    sub_row: u16,
    active_tab: usize,
    mouse_col: Option<u16>,
    mouse_row: Option<u16>,
    selected_count: usize,
) {
    let tab_color = theme::tab_color(active_tab);
    let disabled_color = theme::DISABLED();
    let right_edge = area.x + area.width;

    let mut tx = area.x + LEFT_PAD;
    for (i, tool) in tools.iter().enumerate() {
        let w = sub_tool_width(tool);
        let enabled = !tool.needs_selection || selected_count > 0;
        let is_hovered = mouse_col.is_some_and(|mc| mc >= tx && mc < tx + w)
            && mouse_row.is_some_and(|mr| mr == sub_row);

        let icon_fg = if !enabled {
            disabled_color
        } else if is_hovered {
            theme::TEXT()
        } else {
            tab_color
        };
        let label_fg = if !enabled {
            disabled_color
        } else if is_hovered {
            theme::TEXT()
        } else {
            theme::TEXT_MUTED()
        };

        for ch in tool.icon.chars() {
            if tx >= right_edge {
                break;
            }
            set_char(buf, tx, sub_row, ch, icon_fg, theme::SURFACE());
            tx += 1;
        }
        if tx < right_edge {
            set_char(buf, tx, sub_row, ' ', theme::TEXT(), theme::SURFACE());
            tx += 1;
        }
        for ch in tool.label.chars() {
            if tx >= right_edge {
                break;
            }
            set_char(buf, tx, sub_row, ch, label_fg, theme::SURFACE());
            tx += 1;
        }

        if i + 1 < tools.len() {
            tx += SUB_TOOL_GAP;
        }
    }
}

/// Return the tab index at (col, row), if any.
pub fn tab_at_column(active_area: Rect, col: u16, row: u16) -> Option<usize> {
    let tab_row = active_area.y + TAB_ROW_OFFSET;
    if row != tab_row {
        return None;
    }
    for (i, (icon, label, _)) in theme::TABS.iter().enumerate() {
        let start = tab_start_x(active_area, i);
        let end = start + tab_width(icon, label);
        if col >= start && col < end {
            return Some(i);
        }
    }
    None
}

/// Return the sub-tool index at (col, row), if any.
pub fn sub_tool_at(active_area: Rect, active_tab: usize, col: u16, row: u16) -> Option<usize> {
    let sub_row = active_area.y + SUB_ROW_OFFSET;
    if row != sub_row {
        return None;
    }
    let tools = sub_tools(active_tab);
    if tools.is_empty() {
        return None;
    }
    for (i, tool) in tools.iter().enumerate() {
        let start = sub_tool_start_x(active_area, tools, i);
        let end = start + sub_tool_width(tool);
        if col >= start && col < end {
            return Some(i);
        }
    }
    None
}

/// Rect covering the flush tool strip. Width = full area; height = 2 when the
/// active tab has a sub-row, else 1. Anchored at row 1 under the menu bar.
pub fn toolbar_rect(area: Rect, active_tab: usize) -> Rect {
    let tools = sub_tools(active_tab);
    let height: u16 = if tools.is_empty() { 1 } else { 2 };
    Rect::new(area.x, area.y + TAB_ROW_OFFSET, area.width, height)
}
