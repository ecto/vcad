//! Welcome overlay — shown on first launch when no document is loaded.
//!
//! Renders a centered card with the vcad logo, quick-start actions, and
//! keyboard hints. Dismissed by any listed key or Escape.

use super::buffer::{set_char, set_string, CellBuffer, Rect};
use super::theme;
use vcad_i18n::t;

/// Selected action in the welcome overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WelcomeAction {
    /// Start the guided tutorial (add cube → cylinder → subtract).
    Tutorial,
    /// Create a blank project with an empty document.
    BlankProject,
    /// Open a file (triggers the file picker / command).
    OpenFile,
    /// Dismiss without doing anything.
    Dismiss,
}

/// Draw the welcome overlay centered in `area`.
pub fn draw_welcome(buf: &mut CellBuffer, selected: usize, area: Rect) {
    let card_w: u16 = 42;
    let card_h: u16 = 14;

    if area.width < card_w + 4 || area.height < card_h + 4 {
        // Terminal too small — skip
        return;
    }

    let x = area.x + (area.width.saturating_sub(card_w)) / 2;
    let y = area.y + (area.height.saturating_sub(card_h)) / 2;
    let rect = Rect::new(x, y, card_w, card_h);

    render_card(buf, rect, selected);
}

fn render_card(buf: &mut CellBuffer, area: Rect, selected: usize) {
    let top = area.y;
    let bot = area.y + area.height - 1;
    let left = area.x;
    let right = area.x + area.width - 1;
    let bg = theme::SURFACE();

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_char(buf, x, y, ' ', bg, bg);
        }
    }

    // Box border
    set_char(buf, left, top, '\u{250C}', theme::BORDER(), bg);
    set_char(buf, right, top, '\u{2510}', theme::BORDER(), bg);
    set_char(buf, left, bot, '\u{2514}', theme::BORDER(), bg);
    set_char(buf, right, bot, '\u{2518}', theme::BORDER(), bg);
    for x in (left + 1)..right {
        set_char(buf, x, top, '\u{2500}', theme::BORDER(), bg);
        set_char(buf, x, bot, '\u{2500}', theme::BORDER(), bg);
    }
    for y in (top + 1)..bot {
        set_char(buf, left, y, '\u{2502}', theme::BORDER(), bg);
        set_char(buf, right, y, '\u{2502}', theme::BORDER(), bg);
    }

    let cx = left + 2; // content left margin
    let mut row = top + 1;

    // Logo
    let logo_x = left + (area.width.saturating_sub(5)) / 2;
    set_string(buf, logo_x, row, "vcad", theme::TEXT(), bg);
    set_char(buf, logo_x + 4, row, '.', theme::ACCENT(), bg);
    row += 1;

    // Tagline
    let tagline = t("welcome.tagline");
    let tag_x = left + (area.width.saturating_sub(tagline.chars().count() as u16)) / 2;
    set_string(buf, tag_x, row, tagline, theme::TEXT_MUTED(), bg);
    row += 2;

    // Separator
    for x in (left + 1)..right {
        set_char(buf, x, row, '\u{2500}', theme::BORDER(), bg);
    }
    set_char(buf, left, row, '\u{251C}', theme::BORDER(), bg);
    set_char(buf, right, row, '\u{2524}', theme::BORDER(), bg);
    row += 1;

    // Menu items: (icon, label_key, hint_key)
    let items: &[(&str, &str, &str)] = &[
        ("\u{25B6}", "welcome.new_project", "welcome.hint.new"),
        ("+", "welcome.blank_project", "welcome.hint.blank"),
        ("\u{2192}", "welcome.open_file", "welcome.hint.open"),
    ];

    for (i, (icon, label_key, hint_key)) in items.iter().enumerate() {
        let label = t(label_key);
        let hint = t(hint_key);
        let is_selected = i == selected;
        let row_bg = if is_selected { theme::CARD() } else { bg };

        // Clear row
        for x in (left + 1)..right {
            set_char(buf, x, row, ' ', row_bg, row_bg);
        }

        // Icon
        let mut col = cx;
        for ch in icon.chars() {
            if col < right {
                let color = if is_selected {
                    theme::ACCENT()
                } else {
                    theme::GREEN()
                };
                set_char(buf, col, row, ch, color, row_bg);
                col += 1;
            }
        }
        col += 1;

        // Label
        for ch in label.chars() {
            if col < right {
                set_char(buf, col, row, ch, theme::TEXT(), row_bg);
                col += 1;
            }
        }

        // Right-aligned hint
        let hint_x = right.saturating_sub(hint.chars().count() as u16 + 1);
        if hint_x > col + 1 {
            let mut hx = hint_x;
            for ch in hint.chars() {
                if hx < right {
                    set_char(buf, hx, row, ch, theme::TEXT_MUTED(), row_bg);
                    hx += 1;
                }
            }
        }

        row += 1;
    }

    row += 1;

    // Footer hint
    let footer = t("welcome.footer");
    let footer_x = left + (area.width.saturating_sub(footer.chars().count() as u16)) / 2;
    if row < bot {
        set_string(buf, footer_x, row, footer, theme::TEXT_MUTED(), bg);
    }
}

/// Number of selectable items in the welcome overlay.
pub const ITEM_COUNT: usize = 3;
