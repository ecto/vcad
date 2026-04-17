//! Borland-style menu bar + dropdown popovers.
//!
//! The menu bar is the top row: logo + File/Edit/View/Tools/Help, with each
//! top-level label showing an underlined accelerator letter. Clicking a label
//! or pressing Alt+letter opens a dropdown popover; clicking an item runs the
//! bound text command via `App::process_command`.
//!
//! Layout mirrors `packages/app/src/components/Header.tsx` at TUI fidelity:
//! sharp corners, monospace labels, accent-colored brand dot.

use super::buffer::{set_char, set_char_underline, set_string, CellBuffer, Rect};
use super::theme;

/// A single row in a dropdown.
pub enum MenuItem {
    /// Clickable action — `command` is passed to `App::process_command`.
    Action {
        label: &'static str,
        shortcut: &'static str,
        command: &'static str,
    },
    /// Horizontal separator line.
    Separator,
    /// Submenu that opens to the right (currently rendered inline for M1).
    #[allow(dead_code)]
    Submenu {
        label: &'static str,
        items: &'static [MenuItem],
    },
}

/// Top-level menu metadata.
pub struct Menu {
    pub label: &'static str,
    /// Lowercase accelerator letter (Alt+this opens the menu).
    pub accelerator: char,
    /// Byte index in `label` of the accelerator character. For every
    /// top-level menu currently this is 0 (first letter underlined).
    pub accelerator_index: usize,
    pub items: &'static [MenuItem],
}

const FILE_ITEMS: &[MenuItem] = &[
    MenuItem::Action {
        label: "New",
        shortcut: "Ctrl+N",
        command: "new",
    },
    MenuItem::Action {
        label: "Open…",
        shortcut: "Ctrl+O",
        command: "open",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Save",
        shortcut: "Ctrl+S",
        command: "save",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Export STL",
        shortcut: "",
        command: "export_stl",
    },
    MenuItem::Action {
        label: "Export GLB",
        shortcut: "",
        command: "export_glb",
    },
    MenuItem::Action {
        label: "Export STEP",
        shortcut: "",
        command: "export_step",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Quit",
        shortcut: "Ctrl+Q",
        command: "quit",
    },
];

const EDIT_ITEMS: &[MenuItem] = &[
    MenuItem::Action {
        label: "Undo",
        shortcut: "u",
        command: "undo",
    },
    MenuItem::Action {
        label: "Redo",
        shortcut: "Ctrl+R",
        command: "redo",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Delete",
        shortcut: "x",
        command: "delete",
    },
    MenuItem::Action {
        label: "Duplicate",
        shortcut: "",
        command: "duplicate",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Select All",
        shortcut: "",
        command: "select_all",
    },
    MenuItem::Action {
        label: "Deselect",
        shortcut: "Esc",
        command: "deselect",
    },
];

const VIEW_ITEMS: &[MenuItem] = &[
    MenuItem::Action {
        label: "Toggle Sidebar",
        shortcut: "\\",
        command: "toggle_sidebar",
    },
    MenuItem::Action {
        label: "Toggle Chat",
        shortcut: "~",
        command: "toggle_chat",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Isometric View",
        shortcut: "7",
        command: "camera_iso",
    },
    MenuItem::Action {
        label: "Top View",
        shortcut: "8",
        command: "camera_top",
    },
    MenuItem::Action {
        label: "Front View",
        shortcut: "9",
        command: "camera_front",
    },
    MenuItem::Action {
        label: "Right View",
        shortcut: "0",
        command: "camera_right",
    },
    MenuItem::Action {
        label: "Fit to Screen",
        shortcut: "f",
        command: "camera_fit",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Wireframe",
        shortcut: "",
        command: "toggle_wireframe",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Cycle Theme",
        shortcut: "",
        command: "cycle_theme",
    },
];

const TOOLS_ITEMS: &[MenuItem] = &[
    MenuItem::Action {
        label: "Command Palette…",
        shortcut: ":",
        command: "palette",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "New Sketch",
        shortcut: "S",
        command: "sketch",
    },
];

const HELP_ITEMS: &[MenuItem] = &[
    MenuItem::Action {
        label: "About vcad",
        shortcut: "",
        command: "about",
    },
    MenuItem::Separator,
    MenuItem::Action {
        label: "Open Docs",
        shortcut: "",
        command: "open_docs",
    },
    MenuItem::Action {
        label: "GitHub",
        shortcut: "",
        command: "open_github",
    },
    MenuItem::Action {
        label: "Discord",
        shortcut: "",
        command: "open_discord",
    },
];

/// The five top-level menus, in display order.
pub static MENUS: &[Menu] = &[
    Menu {
        label: "File",
        accelerator: 'f',
        accelerator_index: 0,
        items: FILE_ITEMS,
    },
    Menu {
        label: "Edit",
        accelerator: 'e',
        accelerator_index: 0,
        items: EDIT_ITEMS,
    },
    Menu {
        label: "View",
        accelerator: 'v',
        accelerator_index: 0,
        items: VIEW_ITEMS,
    },
    Menu {
        label: "Tools",
        accelerator: 't',
        accelerator_index: 0,
        items: TOOLS_ITEMS,
    },
    Menu {
        label: "Help",
        accelerator: 'h',
        accelerator_index: 0,
        items: HELP_ITEMS,
    },
];

/// Ephemeral menu-bar state owned by `App`.
#[derive(Debug, Default, Clone)]
pub struct MenuBarState {
    /// Index into `MENUS` of the currently open dropdown, if any.
    pub open: Option<usize>,
    /// Hover/keyboard focus within the open dropdown.
    pub focused_item: usize,
}

impl MenuBarState {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }
    pub fn close(&mut self) {
        self.open = None;
        self.focused_item = 0;
    }
    pub fn open_menu(&mut self, idx: usize) {
        self.open = Some(idx);
        self.focused_item = 0;
    }
}

// ---------- layout constants ----------

const LEFT_PAD: u16 = 1;
const LOGO_WIDTH: u16 = 5; // "vcad" + "."
const LOGO_GAP: u16 = 2; // blank cells between logo and first menu
const LABEL_PAD: u16 = 2; // cells of horizontal padding around each menu label

/// Column where a given menu label's first character lives.
fn menu_label_x(area: Rect, menu_idx: usize) -> u16 {
    let mut x = area.x + LEFT_PAD + LOGO_WIDTH + LOGO_GAP;
    for m in MENUS.iter().take(menu_idx) {
        let label_w = m.label.chars().count() as u16;
        x += label_w + LABEL_PAD * 2;
    }
    x + LABEL_PAD
}

/// Rect covering a top-level menu label's clickable area (label + padding).
pub fn menu_label_rect(area: Rect, menu_idx: usize) -> Rect {
    let label_x = menu_label_x(area, menu_idx);
    let label_w = MENUS[menu_idx].label.chars().count() as u16;
    Rect::new(label_x - LABEL_PAD, area.y, label_w + LABEL_PAD * 2, 1)
}

/// Return the menu index whose label rect contains (col, row), if any.
pub fn menu_at(area: Rect, col: u16, row: u16) -> Option<usize> {
    if row != area.y {
        return None;
    }
    for i in 0..MENUS.len() {
        let r = menu_label_rect(area, i);
        if col >= r.x && col < r.x + r.width {
            return Some(i);
        }
    }
    None
}

/// Compute the rect of an open menu's dropdown popover. The popover hangs
/// directly below the menu label, flush-left with the label padding.
pub fn popover_rect(area: Rect, menu_idx: usize) -> Rect {
    let label_x = menu_label_x(area, menu_idx);
    let items = MENUS[menu_idx].items;
    let max_label_w = items
        .iter()
        .map(|it| match it {
            MenuItem::Action {
                label, shortcut, ..
            } => label.chars().count() + 2 + shortcut.chars().count(),
            MenuItem::Submenu { label, .. } => label.chars().count() + 2,
            MenuItem::Separator => 0,
        })
        .max()
        .unwrap_or(12);
    // +2 for side padding, +2 for border
    let width = (max_label_w + 4).max(14) as u16;
    let height = items.len() as u16 + 2;
    Rect::new(label_x - LABEL_PAD, area.y + 1, width, height)
}

/// Return the item index at (col, row) in an open dropdown, if any.
pub fn item_at(area: Rect, menu_idx: usize, col: u16, row: u16) -> Option<usize> {
    let rect = popover_rect(area, menu_idx);
    if col < rect.x + 1 || col >= rect.x + rect.width - 1 {
        return None;
    }
    if row < rect.y + 1 || row >= rect.y + rect.height - 1 {
        return None;
    }
    let idx = (row - (rect.y + 1)) as usize;
    let items = MENUS[menu_idx].items;
    if idx >= items.len() {
        return None;
    }
    // Separators aren't clickable.
    if matches!(items[idx], MenuItem::Separator) {
        return None;
    }
    Some(idx)
}

// ---------- drawing ----------

/// Draw the top menu-bar row into `area` (y = area.y, height = 1 consumed).
pub fn draw_menu_bar(buf: &mut CellBuffer, area: Rect, state: &MenuBarState) {
    let y = area.y;

    // Fill background across the whole bar.
    for x in area.x..area.x + area.width {
        set_char(buf, x, y, ' ', theme::TEXT(), theme::SURFACE());
    }

    // Logo: "vcad" in text color, trailing "." in brand accent.
    let lx = area.x + LEFT_PAD;
    set_string(buf, lx, y, "vcad", theme::TEXT(), theme::SURFACE());
    set_char(buf, lx + 4, y, '.', theme::ACCENT(), theme::SURFACE());

    // Menu labels.
    for (i, menu) in MENUS.iter().enumerate() {
        let label_x = menu_label_x(area, i);
        let is_open = state.open == Some(i);
        let bg = if is_open {
            theme::BORDER()
        } else {
            theme::SURFACE()
        };
        let fg = theme::TEXT();

        // Redraw padding cells with the highlight bg if open.
        let rect = menu_label_rect(area, i);
        for x in rect.x..rect.x + rect.width {
            set_char(buf, x, y, ' ', fg, bg);
        }

        // Draw each label char, underlining the accelerator.
        for (ci, ch) in menu.label.chars().enumerate() {
            let col = label_x + ci as u16;
            if ci == menu.accelerator_index {
                set_char_underline(buf, col, y, ch, fg, bg);
            } else {
                set_char(buf, col, y, ch, fg, bg);
            }
        }
    }
}

/// Draw the open dropdown, if any. Drawn last so it overlays other widgets.
pub fn draw_open_menu(buf: &mut CellBuffer, area: Rect, state: &MenuBarState) {
    let Some(menu_idx) = state.open else {
        return;
    };
    let menu = &MENUS[menu_idx];
    let rect = popover_rect(area, menu_idx);

    // Bail if the popover can't fit.
    if rect.x + rect.width > buf.width || rect.y + rect.height > buf.height {
        return;
    }

    // Fill background.
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
            set_char(buf, x, y, ' ', theme::TEXT(), theme::SURFACE());
        }
    }

    // Flat border (┌┐└┘ ─ │).
    let left = rect.x;
    let right = rect.x + rect.width - 1;
    let top = rect.y;
    let bot = rect.y + rect.height - 1;
    set_char(
        buf,
        left,
        top,
        '\u{250C}',
        theme::BORDER(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        right,
        top,
        '\u{2510}',
        theme::BORDER(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        left,
        bot,
        '\u{2514}',
        theme::BORDER(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        right,
        bot,
        '\u{2518}',
        theme::BORDER(),
        theme::SURFACE(),
    );
    for x in (left + 1)..right {
        set_char(buf, x, top, '\u{2500}', theme::BORDER(), theme::SURFACE());
        set_char(buf, x, bot, '\u{2500}', theme::BORDER(), theme::SURFACE());
    }
    for y in (top + 1)..bot {
        set_char(buf, left, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
        set_char(buf, right, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
    }

    // Items.
    let inner_w = (rect.width - 2) as usize;
    for (i, item) in menu.items.iter().enumerate() {
        let row = top + 1 + i as u16;
        let is_focused = i == state.focused_item && !matches!(item, MenuItem::Separator);
        let bg = if is_focused {
            theme::BORDER()
        } else {
            theme::SURFACE()
        };
        let fg = theme::TEXT();
        let muted = theme::TEXT_MUTED();

        // Clear row.
        for x in (left + 1)..right {
            set_char(buf, x, row, ' ', fg, bg);
        }

        match item {
            MenuItem::Action {
                label, shortcut, ..
            } => {
                // Label flush-left, shortcut flush-right.
                set_string(buf, left + 2, row, label, fg, bg);
                if !shortcut.is_empty() {
                    let sc_w = shortcut.chars().count() as u16;
                    let sc_x = right - 1 - sc_w;
                    set_string(buf, sc_x, row, shortcut, muted, bg);
                }
            }
            MenuItem::Submenu { label, .. } => {
                set_string(buf, left + 2, row, label, fg, bg);
                set_char(buf, right - 2, row, '\u{25B8}', muted, bg); // ▸
            }
            MenuItem::Separator => {
                for x in (left + 1)..right {
                    set_char(buf, x, row, '\u{2500}', theme::BORDER(), theme::SURFACE());
                }
                // Left/right connectors to the border.
                set_char(
                    buf,
                    left,
                    row,
                    '\u{251C}',
                    theme::BORDER(),
                    theme::SURFACE(),
                );
                set_char(
                    buf,
                    right,
                    row,
                    '\u{2524}',
                    theme::BORDER(),
                    theme::SURFACE(),
                );
            }
        }
        // Silence unused-variable warning for inner_w; reserved for future
        // label truncation work.
        let _ = inner_w;
    }
}

/// Resolve an Alt+letter event to a menu index, if any.
pub fn accelerator_to_menu(ch: char) -> Option<usize> {
    let lower = ch.to_ascii_lowercase();
    MENUS.iter().position(|m| m.accelerator == lower)
}

/// Fetch a command string bound to the nth item of the open menu.
pub fn item_command(menu_idx: usize, item_idx: usize) -> Option<&'static str> {
    let items = MENUS.get(menu_idx)?.items;
    match items.get(item_idx)? {
        MenuItem::Action { command, .. } => Some(*command),
        _ => None,
    }
}
