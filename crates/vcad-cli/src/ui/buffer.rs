//! Custom cell buffer for terminal UI rendering.
//!
//! Replaces ratatui with a minimal diff-based buffer that emits only changed cells.

use crossterm::{
    cursor::MoveTo,
    style::{Attribute, Color as CtColor, SetAttribute, SetBackgroundColor, SetForegroundColor},
    QueueableCommand,
};
use std::io::{self, Write};

use super::theme;

/// Screen rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl Rect {
    pub fn new(x: u16, y: u16, width: u16, height: u16) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

/// 16 ANSI named colors — inherit whatever palette the user's terminal
/// has configured. Mirrors the subset of `crossterm::style::Color`
/// variants that are safe on every terminal. Variants that aren't
/// currently referenced by any theme are still part of the palette so
/// call sites (and future themes) can reach for them.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedColor {
    Black,
    DarkGrey,
    Grey,
    White,
    Red,
    DarkRed,
    Green,
    DarkGreen,
    Yellow,
    DarkYellow,
    Blue,
    DarkBlue,
    Magenta,
    DarkMagenta,
    Cyan,
    DarkCyan,
}

/// A cell color. Three kinds:
///
/// - [`Color::Default`] — use the terminal's current fg/bg (SGR 39/49).
///   Picked by the `Terminal` theme so the TUI inherits the user's
///   existing terminal colors instead of imposing our own.
/// - [`Color::Rgb`] — a 24-bit true-color value. Used by the viewport
///   rasterizer and by the Dark/Light themes.
/// - [`Color::Named`] — one of the 16 ANSI named colors. Used by the
///   Terminal theme for semantic accents (brand = Red, warn = Yellow,
///   success = Green, …) so each resolves against the terminal's
///   palette.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Color {
    Default,
    Rgb(u8, u8, u8),
    Named(NamedColor),
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Color::Rgb(r, g, b)
    }
}

fn to_crossterm_color(c: Color) -> CtColor {
    match c {
        Color::Default => CtColor::Reset,
        Color::Rgb(r, g, b) => CtColor::Rgb { r, g, b },
        Color::Named(n) => match n {
            NamedColor::Black => CtColor::Black,
            NamedColor::DarkGrey => CtColor::DarkGrey,
            NamedColor::Grey => CtColor::Grey,
            NamedColor::White => CtColor::White,
            NamedColor::Red => CtColor::Red,
            NamedColor::DarkRed => CtColor::DarkRed,
            NamedColor::Green => CtColor::Green,
            NamedColor::DarkGreen => CtColor::DarkGreen,
            NamedColor::Yellow => CtColor::Yellow,
            NamedColor::DarkYellow => CtColor::DarkYellow,
            NamedColor::Blue => CtColor::Blue,
            NamedColor::DarkBlue => CtColor::DarkBlue,
            NamedColor::Magenta => CtColor::Magenta,
            NamedColor::DarkMagenta => CtColor::DarkMagenta,
            NamedColor::Cyan => CtColor::Cyan,
            NamedColor::DarkCyan => CtColor::DarkCyan,
        },
    }
}

/// A single terminal cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    /// SGR underline flag — used by menu-bar accelerators and link styling.
    pub underline: bool,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: theme::TEXT(),
            bg: theme::BG(),
            underline: false,
        }
    }
}

/// Double-buffered cell grid with diff-based flushing.
pub struct CellBuffer {
    pub width: u16,
    pub height: u16,
    cells: Vec<Cell>,
    prev: Vec<Cell>,
}

impl CellBuffer {
    /// Allocate a new buffer.
    ///
    /// Both `cells` and `prev` are initialized to `Cell::default()` so that on the first
    /// flush only cells actively written by overlays will diff as changed.
    /// This prevents pixel-protocol images (Kitty/Sixel) from being overwritten with spaces.
    pub fn new(width: u16, height: u16) -> Self {
        let size = width as usize * height as usize;
        Self {
            width,
            height,
            cells: vec![Cell::default(); size],
            prev: vec![Cell::default(); size],
        }
    }

    /// Get a mutable reference to a cell.
    pub fn cell_mut(&mut self, x: u16, y: u16) -> Option<&mut Cell> {
        if x < self.width && y < self.height {
            let idx = y as usize * self.width as usize + x as usize;
            self.cells.get_mut(idx)
        } else {
            None
        }
    }

    /// Clear all cells to a background color.
    #[allow(dead_code)]
    pub fn clear(&mut self, bg: Color) {
        for cell in &mut self.cells {
            cell.ch = ' ';
            cell.fg = theme::TEXT();
            cell.bg = bg;
        }
    }

    /// Flush only changed cells to the terminal.
    pub fn flush(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        // Sentinel — `Default` is the first color `to_crossterm_color`
        // resolves to `Reset`, so the very first non-default cell will
        // correctly force an SGR write.
        let mut last_fg = Color::Default;
        let mut last_bg = Color::Default;
        let mut last_underline = false;
        let mut last_pos: Option<(u16, u16)> = None;
        let mut colors_set = false;

        for y in 0..self.height {
            for x in 0..self.width {
                let idx = y as usize * self.width as usize + x as usize;
                if self.cells[idx] == self.prev[idx] {
                    continue;
                }

                let cell = &self.cells[idx];

                // Move cursor if not sequential
                let need_move = last_pos.is_none_or(|(lx, ly)| ly != y || lx + 1 != x);
                if need_move {
                    stdout.queue(MoveTo(x, y))?;
                }

                // Set colors if changed
                if !colors_set || cell.fg != last_fg {
                    stdout.queue(SetForegroundColor(to_crossterm_color(cell.fg)))?;
                    last_fg = cell.fg;
                }
                if !colors_set || cell.bg != last_bg {
                    stdout.queue(SetBackgroundColor(to_crossterm_color(cell.bg)))?;
                    last_bg = cell.bg;
                }
                if !colors_set || cell.underline != last_underline {
                    stdout.queue(SetAttribute(if cell.underline {
                        Attribute::Underlined
                    } else {
                        Attribute::NoUnderline
                    }))?;
                    last_underline = cell.underline;
                }
                colors_set = true;

                // Write character
                write!(stdout, "{}", cell.ch)?;
                last_pos = Some((x, y));
            }
        }

        // Reset attributes at the end of the frame so external terminal state
        // (e.g. the shell prompt after we exit alt-screen) doesn't inherit them.
        stdout.queue(SetAttribute(Attribute::Reset))?;
        stdout.flush()?;

        // Swap buffers
        std::mem::swap(&mut self.cells, &mut self.prev);
        // Reset current buffer from prev (which now has the displayed state)
        self.cells.clone_from(&self.prev);

        Ok(())
    }

    /// Handle terminal resize.
    pub fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        let size = width as usize * height as usize;
        self.cells = vec![Cell::default(); size];
        self.prev = vec![Cell::default(); size];
    }
}

/// Swap any C0/DEL control character for a visible placeholder. The
/// terminal interprets raw `\n`, `\r`, escape, etc. as real control
/// codes and would corrupt the display if we wrote them into the cell
/// buffer verbatim — this keeps stray log text from an upstream server
/// or a library panic from escaping the TUI's own rendering.
fn safe_glyph(ch: char) -> char {
    let code = ch as u32;
    if (code < 0x20) || code == 0x7F {
        '\u{00B7}' // ·
    } else {
        ch
    }
}

/// Helper: set a character with colors at a position. Clears any underline
/// attribute — call `set_char_underline` if you want it.
pub fn set_char(buf: &mut CellBuffer, x: u16, y: u16, ch: char, fg: Color, bg: Color) {
    if let Some(cell) = buf.cell_mut(x, y) {
        cell.ch = safe_glyph(ch);
        cell.fg = fg;
        cell.bg = bg;
        cell.underline = false;
    }
}

/// Helper: set an underlined character. Used by menu-bar accelerators.
pub fn set_char_underline(buf: &mut CellBuffer, x: u16, y: u16, ch: char, fg: Color, bg: Color) {
    if let Some(cell) = buf.cell_mut(x, y) {
        cell.ch = safe_glyph(ch);
        cell.fg = fg;
        cell.bg = bg;
        cell.underline = true;
    }
}

/// Helper: write a string horizontally starting at (x, y).
pub fn set_string(buf: &mut CellBuffer, x: u16, y: u16, s: &str, fg: Color, bg: Color) {
    let mut cx = x;
    for ch in s.chars() {
        if cx >= buf.width {
            break;
        }
        set_char(buf, cx, y, ch, fg, bg);
        cx += 1;
    }
}
