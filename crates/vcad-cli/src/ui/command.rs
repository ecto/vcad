//! Command palette overlay.

use super::buffer::{set_char, CellBuffer, Rect};
use super::theme;

/// Draw the command palette overlay.
pub fn draw_command_palette(
    buf: &mut CellBuffer,
    input: &str,
    items: &[CommandItem],
    selected_index: usize,
    area: Rect,
) {
    let palette_width = 44u16.min(area.width.saturating_sub(4));
    let item_count = items.len().min(8);
    let palette_height = (item_count + 3) as u16;

    let x = area.x + (area.width.saturating_sub(palette_width)) / 2;
    let y = area.y + area.height.saturating_sub(palette_height + 5);

    let rect = Rect::new(x, y, palette_width, palette_height);

    render_palette(buf, rect, input, items, selected_index);
}

/// A single item in the command palette.
#[derive(Debug, Clone)]
pub struct CommandItem {
    /// Canonical command id (matches `vcad_app::commands::Command::id` and
    /// `App::process_command`'s match arms). Used for dispatch.
    pub id: String,
    pub icon: String,
    pub label: String,
    pub description: String,
}

fn render_palette(
    buf: &mut CellBuffer,
    area: Rect,
    input: &str,
    items: &[CommandItem],
    selected_index: usize,
) {
    if area.height < 3 || area.width < 10 {
        return;
    }

    let top = area.y;
    let bot = area.y + area.height - 1;
    let left = area.x;
    let right = area.x + area.width - 1;

    // Fill background
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            set_char(buf, x, y, ' ', theme::SURFACE(), theme::SURFACE());
        }
    }

    // Rounded border
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

    // Search input row
    let search_y = top + 1;
    let inner_left = left + 1;
    let inner_right = right;

    set_char(
        buf,
        inner_left + 1,
        search_y,
        '>',
        theme::ACCENT(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        inner_left + 2,
        search_y,
        ' ',
        theme::SURFACE(),
        theme::SURFACE(),
    );

    for (i, ch) in input.chars().enumerate() {
        let x = inner_left + 3 + i as u16;
        if x < inner_right {
            set_char(buf, x, search_y, ch, theme::TEXT(), theme::SURFACE());
        }
    }

    // Cursor
    let cursor_x = inner_left + 3 + input.len() as u16;
    if cursor_x < inner_right {
        set_char(
            buf,
            cursor_x,
            search_y,
            '\u{2588}',
            theme::ACCENT(),
            theme::SURFACE(),
        );
    }

    // "esc" hint right-aligned
    let esc_text = "esc";
    let esc_x = inner_right.saturating_sub(esc_text.len() as u16 + 1);
    for (i, ch) in esc_text.chars().enumerate() {
        set_char(
            buf,
            esc_x + i as u16,
            search_y,
            ch,
            theme::TEXT_MUTED(),
            theme::SURFACE(),
        );
    }

    // Separator line
    if area.height > 3 {
        let sep_y = search_y + 1;
        for x in (left + 1)..right {
            set_char(buf, x, sep_y, '\u{2500}', theme::BORDER(), theme::SURFACE());
        }
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

        // Items
        let items_start_y = sep_y + 1;
        for (i, item) in items.iter().enumerate() {
            let y = items_start_y + i as u16;
            if y >= bot {
                break;
            }

            let is_selected = i == selected_index;
            let row_bg = if is_selected {
                theme::CARD()
            } else {
                theme::SURFACE()
            };

            // Clear row
            for x in (left + 1)..right {
                set_char(buf, x, y, ' ', row_bg, row_bg);
            }

            // Icon
            let mut cx = inner_left + 1;
            for ch in item.icon.chars() {
                if cx < inner_right {
                    set_char(buf, cx, y, ch, theme::GREEN(), row_bg);
                    cx += 1;
                }
            }

            cx += 1; // space

            // Label
            for ch in item.label.chars() {
                if cx < inner_right {
                    set_char(buf, cx, y, ch, theme::TEXT(), row_bg);
                    cx += 1;
                }
            }

            // Description right-aligned
            if !item.description.is_empty() {
                let desc_x = inner_right.saturating_sub(item.description.len() as u16 + 1);
                if desc_x > cx + 1 {
                    for (j, ch) in item.description.chars().enumerate() {
                        set_char(buf, desc_x + j as u16, y, ch, theme::TEXT_MUTED(), row_bg);
                    }
                }
            }
        }
    }
}

/// Build command items from the search query.
pub fn build_command_items(query: &str) -> Vec<CommandItem> {
    let commands = vcad_app::commands::find_commands(query);
    commands
        .into_iter()
        .take(8)
        .map(|cmd| CommandItem {
            id: cmd.id.to_string(),
            icon: cmd.icon.to_string(),
            label: cmd.label().to_string(),
            description: cmd.shortcut.unwrap_or("").to_string(),
        })
        .collect()
}

/// Returns the Rect of the command palette for hit-testing.
pub fn palette_rect(area: Rect, item_count: usize) -> Rect {
    let palette_width = 44u16.min(area.width.saturating_sub(4));
    let palette_height = (item_count.min(8) + 3) as u16;

    let x = area.x + (area.width.saturating_sub(palette_width)) / 2;
    let y = area.y + area.height.saturating_sub(palette_height + 5);

    Rect::new(x, y, palette_width, palette_height)
}

/// Returns the item index at the given row, if any.
pub fn item_at_row(area: Rect, item_count: usize, row: u16) -> Option<usize> {
    let rect = palette_rect(area, item_count);
    let items_start = rect.y + 3;
    if row >= items_start && row < rect.y + rect.height - 1 {
        let idx = (row - items_start) as usize;
        if idx < item_count.min(8) {
            return Some(idx);
        }
    }
    None
}
