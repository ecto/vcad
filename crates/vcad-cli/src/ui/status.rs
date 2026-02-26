//! Status bar — always-visible bottom row.

use super::buffer::{set_char, CellBuffer, Rect};
use super::theme;

/// Draw the status bar at the bottom of the terminal.
pub fn draw_status_bar(
    buf: &mut CellBuffer,
    status: &str,
    parts_count: usize,
    tri_count: usize,
    selected_count: usize,
    area: Rect,
) {
    let bar_y = area.y + area.height - 1;

    // Fill background
    for x in area.x..area.x + area.width {
        set_char(buf, x, bar_y, ' ', theme::SURFACE(), theme::SURFACE());
    }

    let mut cx = area.x + 1;

    // Status message
    let status_color = if status.starts_with("Error") {
        theme::ACCENT()
    } else if status.starts_with("Warning") {
        theme::YELLOW()
    } else {
        theme::GREEN()
    };

    for ch in status.chars() {
        if cx >= area.x + area.width - 1 {
            break;
        }
        set_char(buf, cx, bar_y, ch, status_color, theme::SURFACE());
        cx += 1;
    }

    // Build right-side metrics
    let tri_str = format_count(tri_count);
    let metrics = format!(
        "{} parts \u{2502} {} tris \u{2502} {} selected",
        parts_count, tri_str, selected_count
    );

    // Right-align metrics
    let metrics_start = area.x + area.width.saturating_sub(metrics.len() as u16 + 1);

    // Separator
    if metrics_start > cx + 2 {
        set_char(
            buf,
            cx + 1,
            bar_y,
            '\u{2502}',
            theme::BORDER(),
            theme::SURFACE(),
        );
    }

    // Metrics
    let mut mx = metrics_start;
    for ch in metrics.chars() {
        if mx >= area.x + area.width {
            break;
        }
        let is_digit = ch.is_ascii_digit() || ch == '.' || ch == 'K';
        let color = if ch == '\u{2502}' {
            theme::BORDER()
        } else if is_digit {
            theme::TEXT()
        } else {
            theme::TEXT_MUTED()
        };
        set_char(buf, mx, bar_y, ch, color, theme::SURFACE());
        mx += 1;
    }
}

/// Format a count with K suffix for readability.
fn format_count(n: usize) -> String {
    if n >= 1000 {
        format!("{:.1}K", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_count() {
        assert_eq!(format_count(42), "42");
        assert_eq!(format_count(1200), "1.2K");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(10500), "10.5K");
    }
}
