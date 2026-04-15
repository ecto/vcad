//! Right-docked chat sidebar — conversational AI + debug logs.
//!
//! Layout mirrors `packages/app/src/components/ChatSidebar.tsx`: a
//! full-height column on the right, anchored below the menu bar and above
//! the status bar. Messages stack top-to-bottom with the input line pinned
//! to the bottom.

use super::buffer::{set_char, CellBuffer, Rect};
use super::theme;

/// Maximum number of lines retained in scrollback.
const MAX_LINES: usize = 500;

/// Kind of chat line, determines styling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatLineKind {
    /// User message.
    User,
    /// AI assistant response.
    Assistant,
    /// Debug / system log.
    Debug,
}

/// A single line in the chat panel.
#[derive(Debug, Clone)]
pub struct ChatLine {
    pub text: String,
    pub kind: ChatLineKind,
}

/// Persistent chat sidebar state.
pub struct ChatPanel {
    /// Whether the sidebar is rendered.
    pub open: bool,
    /// Whether the input has keyboard focus — when true, key events route
    /// to the chat input instead of the viewport/menu.
    pub focused: bool,
    /// Current input buffer.
    pub input: String,
    /// Chat lines (messages + debug logs).
    pub lines: Vec<ChatLine>,
    /// Message history for Up/Down recall (user messages only).
    pub history: Vec<String>,
    /// Index into history for Up/Down navigation (`None` = editing new input).
    pub history_index: Option<usize>,
    /// Scroll offset (0 = bottom, increases upward).
    pub scroll: usize,
    /// Saved input when navigating history.
    saved_input: String,
}

impl ChatPanel {
    /// Create a chat panel docked to the right. Open but not focused by
    /// default — so CAD hotkeys still work until the user explicitly
    /// focuses the input via `` ` `` / Tab / clicking the sidebar.
    pub fn new() -> Self {
        Self {
            open: true,
            focused: false,
            input: String::new(),
            lines: Vec::new(),
            history: Vec::new(),
            history_index: None,
            scroll: 0,
            saved_input: String::new(),
        }
    }

    /// Push a line, capping at MAX_LINES.
    fn push_line(&mut self, text: String, kind: ChatLineKind) {
        self.lines.push(ChatLine { text, kind });
        if self.lines.len() > MAX_LINES {
            self.lines.remove(0);
        }
    }

    /// Log a debug/system message.
    pub fn debug(&mut self, msg: impl Into<String>) {
        self.push_line(msg.into(), ChatLineKind::Debug);
    }

    /// Navigate history up.
    pub fn history_up(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.saved_input = self.input.clone();
                let idx = self.history.len() - 1;
                self.history_index = Some(idx);
                self.input = self.history[idx].clone();
            }
            Some(idx) if idx > 0 => {
                let new_idx = idx - 1;
                self.history_index = Some(new_idx);
                self.input = self.history[new_idx].clone();
            }
            _ => {}
        }
    }

    /// Navigate history down.
    pub fn history_down(&mut self) {
        if let Some(idx) = self.history_index {
            if idx + 1 < self.history.len() {
                let new_idx = idx + 1;
                self.history_index = Some(new_idx);
                self.input = self.history[new_idx].clone();
            } else {
                self.history_index = None;
                self.input = self.saved_input.clone();
            }
        }
    }

    /// Submit the current input as a user message.
    /// Returns the message text (for the caller to act on).
    pub fn send_message(&mut self) -> Option<String> {
        let msg = self.input.trim().to_string();
        if msg.is_empty() {
            return None;
        }
        self.push_line(msg.clone(), ChatLineKind::User);
        self.history.push(msg.clone());
        self.history_index = None;
        self.saved_input.clear();
        self.input.clear();
        self.scroll = 0;
        Some(msg)
    }

    /// Add an assistant response line.
    pub fn assistant(&mut self, msg: impl Into<String>) {
        self.push_line(msg.into(), ChatLineKind::Assistant);
    }
}

/// Desired sidebar width in cells. Clamped to 60% of the area width so the
/// viewport still has room on narrow terminals.
const SIDEBAR_WIDTH: u16 = 50;

/// Rows reserved at the top for the menu bar (1) + tool strip (up to 2).
/// The chat sidebar starts immediately below this and the tool strip is
/// rendered across the full width so its right edge is visually occluded
/// by the chat, matching ChatSidebar.tsx's layout under Header.tsx.
const TOP_OFFSET: u16 = 3;
/// Rows reserved at the bottom for the status bar.
const BOTTOM_OFFSET: u16 = 1;

/// Compute the right-docked sidebar rect. Spans from the row below the tool
/// strip down to the row above the status bar.
pub fn chat_rect(area: Rect) -> Rect {
    let max_width = (area.width * 3 / 5).max(20);
    let width = SIDEBAR_WIDTH.min(max_width);
    let x = area.x + area.width.saturating_sub(width);
    let y = area.y + TOP_OFFSET;
    let height = area
        .height
        .saturating_sub(TOP_OFFSET + BOTTOM_OFFSET);
    Rect::new(x, y, width, height)
}

/// Draw the chat panel overlay.
pub fn draw_chat(buf: &mut CellBuffer, panel: &ChatPanel, area: Rect) {
    let rect = chat_rect(area);
    if rect.height < 4 || rect.width < 10 {
        return;
    }

    let top = rect.y;
    let bot = rect.y + rect.height - 1;
    let left = rect.x;
    let right = rect.x + rect.width - 1;

    // Fill background
    for y in rect.y..rect.y + rect.height {
        for x in rect.x..rect.x + rect.width {
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

    // Title in top border
    let title = " Chat ";
    let title_x = left + 2;
    for (i, ch) in title.chars().enumerate() {
        let x = title_x + i as u16;
        if x < right {
            set_char(buf, x, top, ch, theme::ACCENT(), theme::SURFACE());
        }
    }

    // Separator above input line
    let input_sep_y = bot - 1;
    let input_y = bot;

    if input_sep_y > top + 1 {
        for x in (left + 1)..right {
            set_char(
                buf,
                x,
                input_sep_y,
                '\u{2500}',
                theme::BORDER(),
                theme::SURFACE(),
            );
        }
        set_char(
            buf,
            left,
            input_sep_y,
            '\u{251C}',
            theme::BORDER(),
            theme::SURFACE(),
        );
        set_char(
            buf,
            right,
            input_sep_y,
            '\u{2524}',
            theme::BORDER(),
            theme::SURFACE(),
        );
    }

    // Output area: from top+1 to input_sep_y
    let output_top = top + 1;
    let output_bot = if input_sep_y > top + 1 {
        input_sep_y
    } else {
        output_top
    };
    let visible_lines = (output_bot - output_top) as usize;

    if visible_lines > 0 && !panel.lines.is_empty() {
        let total = panel.lines.len();
        let scroll = panel.scroll.min(total.saturating_sub(visible_lines));
        let start = total.saturating_sub(visible_lines + scroll);
        let end = total.saturating_sub(scroll);

        for (i, line) in panel.lines[start..end].iter().enumerate() {
            let y = output_top + i as u16;
            if y >= output_bot {
                break;
            }

            let (prefix, prefix_fg, text_fg) = match line.kind {
                ChatLineKind::User => ("\u{25B6} ", theme::GREEN(), theme::TEXT()),
                ChatLineKind::Assistant => ("\u{2726} ", theme::ACCENT(), theme::PURPLE()),
                ChatLineKind::Debug => ("\u{2502} ", theme::BORDER(), theme::TEXT_MUTED()),
            };

            let inner_w = (right - left - 1) as usize;
            let mut cx = left + 1;

            // Draw prefix
            for ch in prefix.chars() {
                if (cx - left) as usize >= inner_w {
                    break;
                }
                set_char(buf, cx, y, ch, prefix_fg, theme::SURFACE());
                cx += 1;
            }

            // Draw text
            for ch in line.text.chars() {
                if (cx - left) as usize >= inner_w {
                    break;
                }
                set_char(buf, cx, y, ch, text_fg, theme::SURFACE());
                cx += 1;
            }
        }

        // Scroll indicator
        if scroll > 0 {
            let indicator = format!("[+{}]", scroll);
            let ix = right.saturating_sub(indicator.len() as u16 + 1);
            for (j, ch) in indicator.chars().enumerate() {
                set_char(
                    buf,
                    ix + j as u16,
                    output_top,
                    ch,
                    theme::TEXT_MUTED(),
                    theme::SURFACE(),
                );
            }
        }
    }

    // Input line on bottom border row
    for x in (left + 1)..right {
        set_char(buf, x, input_y, ' ', theme::SURFACE(), theme::SURFACE());
    }
    set_char(
        buf,
        left,
        input_y,
        '\u{2514}',
        theme::BORDER(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        right,
        input_y,
        '\u{2518}',
        theme::BORDER(),
        theme::SURFACE(),
    );

    // Prompt
    let inner_left = left + 1;
    let inner_right = right;
    set_char(
        buf,
        inner_left,
        input_y,
        '>',
        theme::ACCENT(),
        theme::SURFACE(),
    );
    set_char(
        buf,
        inner_left + 1,
        input_y,
        ' ',
        theme::SURFACE(),
        theme::SURFACE(),
    );

    // Input text
    let max_input = (inner_right - inner_left - 2) as usize;
    for (i, ch) in panel.input.chars().take(max_input).enumerate() {
        set_char(
            buf,
            inner_left + 2 + i as u16,
            input_y,
            ch,
            theme::TEXT(),
            theme::SURFACE(),
        );
    }

    // Cursor
    let cursor_x = inner_left + 2 + panel.input.len().min(max_input) as u16;
    if cursor_x < inner_right {
        set_char(
            buf,
            cursor_x,
            input_y,
            '\u{2588}',
            theme::ACCENT(),
            theme::SURFACE(),
        );
    }

    // Hint
    let hint = "`/Esc close";
    let hint_x = inner_right.saturating_sub(hint.len() as u16 + 1);
    if hint_x > cursor_x + 2 {
        for (j, ch) in hint.chars().enumerate() {
            set_char(
                buf,
                hint_x + j as u16,
                input_y,
                ch,
                theme::TEXT_MUTED(),
                theme::SURFACE(),
            );
        }
    }
}
