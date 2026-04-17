//! Status bar — three-segment bottom strip matching `StatusBar.tsx`.
//!
//! Layout (flush with the bottom row):
//! ```text
//! │ ⏻ INFO status  saved & sync…               2s │ x  10.5 y  20.3 z   0.0 │ ● modified  3 parts  2 sel │
//! ```
//!
//! - Left: console log ticker — LEVEL in level-color, source muted, message
//!   in text color, right-aligned "Ns ago" timestamp. Fills remaining width.
//! - Middle: live cursor XYZ world coordinates, pink axis labels. Shows `—`
//!   when `app.cursor_world` is None.
//! - Right: save-state pill + parts count + selection count.

use std::time::Instant;

use super::buffer::{set_char, set_string, CellBuffer, Rect};
use super::theme;

use crate::app::{App, LogLevel};

/// Draw the status bar across the bottom row of `area`.
pub fn draw_status_bar(buf: &mut CellBuffer, app: &App, area: Rect) {
    if area.width < 20 {
        return;
    }
    let y = area.y + area.height - 1;

    // Background fill.
    for x in area.x..area.x + area.width {
        set_char(buf, x, y, ' ', theme::TEXT_MUTED(), theme::SURFACE());
    }

    // Right segment — compute first because middle is positioned relative to it.
    let right_w = draw_right_segment(buf, app, area, y);

    // Middle segment — cursor coordinates (fixed-width cluster).
    let middle_text = format_cursor_text(app);
    let middle_w = middle_text.chars().count() as u16;
    let middle_x = (area.x + area.width).saturating_sub(right_w + middle_w + 4);

    // Separator between middle and right.
    if right_w > 0 && middle_w > 0 {
        let sep_x = (area.x + area.width).saturating_sub(right_w + 2);
        set_char(buf, sep_x, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
    }

    // Draw the middle segment.
    draw_cursor_segment(buf, app, middle_x, y, middle_w);

    // Separator between left (ticker) and middle.
    if middle_w > 0 {
        let sep_x = middle_x.saturating_sub(2);
        if sep_x > area.x + 4 {
            set_char(buf, sep_x, y, '\u{2502}', theme::BORDER(), theme::SURFACE());
        }
    }

    // Left segment — log ticker, gets all remaining space.
    let ticker_left = area.x + 1;
    let ticker_right = middle_x.saturating_sub(3).min(area.x + area.width - 1);
    if ticker_right > ticker_left + 4 {
        draw_ticker_segment(buf, app, ticker_left, ticker_right, y);
    }
}

// ---------------------------------------------------------------------------
// Left: console log ticker
// ---------------------------------------------------------------------------

fn draw_ticker_segment(buf: &mut CellBuffer, app: &App, x_start: u16, x_end: u16, y: u16) {
    let now = Instant::now();
    let Some(entry) = app.latest_log() else {
        set_string(
            buf,
            x_start,
            y,
            "console empty",
            theme::TEXT_MUTED(),
            theme::SURFACE(),
        );
        return;
    };

    let level_color = match entry.level {
        LogLevel::Debug => theme::TEXT_MUTED(),
        LogLevel::Info => theme::CYAN(),
        LogLevel::Warn => theme::YELLOW(),
        LogLevel::Error => theme::ACCENT(),
    };

    // LEVEL source message … 2s (right-aligned)
    let mut cx = x_start;
    let level_label = entry.level.label();

    // Level pill.
    for ch in level_label.chars() {
        if cx >= x_end {
            return;
        }
        set_char(buf, cx, y, ch, level_color, theme::SURFACE());
        cx += 1;
    }
    if cx < x_end {
        set_char(buf, cx, y, ' ', theme::TEXT(), theme::SURFACE());
        cx += 1;
    }

    // Source label (muted).
    for ch in entry.source.chars() {
        if cx >= x_end {
            return;
        }
        set_char(buf, cx, y, ch, theme::TEXT_MUTED(), theme::SURFACE());
        cx += 1;
    }
    if cx < x_end {
        set_char(buf, cx, y, ' ', theme::TEXT(), theme::SURFACE());
        cx += 1;
    }

    // Right-aligned age.
    let age = format_ago(entry.timestamp, now);
    let age_w = age.chars().count() as u16;
    let age_x = x_end.saturating_sub(age_w);

    // Message text (clipped against age position).
    let msg_end = age_x.saturating_sub(2);
    for ch in entry.message.chars() {
        if cx >= msg_end {
            break;
        }
        set_char(buf, cx, y, ch, theme::TEXT(), theme::SURFACE());
        cx += 1;
    }

    if age_x < x_end {
        set_string(buf, age_x, y, &age, theme::TEXT_MUTED(), theme::SURFACE());
    }
}

fn format_ago(ts: Instant, now: Instant) -> String {
    let diff = now.saturating_duration_since(ts).as_millis();
    if diff < 2_000 {
        "now".to_string()
    } else if diff < 60_000 {
        format!("{}s", diff / 1000)
    } else if diff < 3_600_000 {
        format!("{}m", diff / 60_000)
    } else {
        format!("{}h", diff / 3_600_000)
    }
}

// ---------------------------------------------------------------------------
// Middle: cursor XYZ
// ---------------------------------------------------------------------------

fn format_cursor_text(_app: &App) -> String {
    // Purely for width measurement — the real render writes individual
    // colored runs in draw_cursor_segment. Width must match exactly.
    "x         y         z        ".to_string()
}

fn draw_cursor_segment(buf: &mut CellBuffer, app: &App, x_start: u16, y: u16, width: u16) {
    if width == 0 {
        return;
    }
    let right_edge = x_start + width;
    let mut cx = x_start;

    for (axis_char, value) in ["x", "y", "z"].iter().enumerate().map(|(i, a)| {
        let v = app.cursor_world.map(|c| match i {
            0 => c.0,
            1 => c.1,
            _ => c.2,
        });
        (*a, v)
    }) {
        // "x" in pink + space + number in text (or placeholder).
        if cx >= right_edge {
            break;
        }
        for ch in axis_char.chars() {
            if cx >= right_edge {
                break;
            }
            set_char(buf, cx, y, ch, theme::ACCENT(), theme::SURFACE());
            cx += 1;
        }
        let formatted = match value {
            Some(v) => format!("{:>8.1}", v),
            None => "       —".to_string(),
        };
        for ch in formatted.chars() {
            if cx >= right_edge {
                break;
            }
            let fg = if value.is_some() {
                theme::TEXT()
            } else {
                theme::DISABLED()
            };
            set_char(buf, cx, y, ch, fg, theme::SURFACE());
            cx += 1;
        }
        if cx < right_edge {
            set_char(buf, cx, y, ' ', theme::TEXT(), theme::SURFACE());
            cx += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Right: save state + parts + selection
// ---------------------------------------------------------------------------

/// Draws the right-aligned cluster and returns its total width in cells.
fn draw_right_segment(buf: &mut CellBuffer, app: &App, area: Rect, y: u16) -> u16 {
    // Build text pieces.
    let parts_count = app.get_parts().len();
    let sel_count = app.selected.len();
    let save_label = if app.is_dirty() { "modified" } else { "saved" };
    let save_color = if app.is_dirty() {
        theme::ACCENT()
    } else {
        theme::TEXT_MUTED()
    };

    let parts_text = format!(
        "{parts_count} {}",
        if parts_count == 1 { "part" } else { "parts" }
    );
    let sel_text = if sel_count > 0 {
        format!("{sel_count} sel")
    } else {
        String::new()
    };

    // Total width = "●  " + save_label + "  " + parts_text + maybe "  " + sel_text + right pad 1
    let mut total = 2 + save_label.chars().count() + 2 + parts_text.chars().count() + 1;
    if !sel_text.is_empty() {
        total += 2 + sel_text.chars().count();
    }
    let total_w = total as u16;

    if area.width <= total_w + 4 {
        return 0;
    }
    let mut cx = area.x + area.width - total_w;

    // Save pill: ● + space + label
    set_char(buf, cx, y, '\u{25CF}', save_color, theme::SURFACE());
    cx += 1;
    set_char(buf, cx, y, ' ', theme::TEXT(), theme::SURFACE());
    cx += 1;
    for ch in save_label.chars() {
        set_char(buf, cx, y, ch, save_color, theme::SURFACE());
        cx += 1;
    }
    // Gap
    cx += 2;
    // Parts
    for ch in parts_text.chars() {
        set_char(buf, cx, y, ch, theme::TEXT_MUTED(), theme::SURFACE());
        cx += 1;
    }
    // Selection (when > 0) in accent
    if !sel_text.is_empty() {
        cx += 2;
        for ch in sel_text.chars() {
            set_char(buf, cx, y, ch, theme::ACCENT(), theme::SURFACE());
            cx += 1;
        }
    }

    total_w
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_ago() {
        let now = Instant::now();
        assert_eq!(format_ago(now, now), "now");
        assert_eq!(
            format_ago(now - std::time::Duration::from_secs(5), now),
            "5s"
        );
        assert_eq!(
            format_ago(now - std::time::Duration::from_secs(120), now),
            "2m"
        );
    }
}
