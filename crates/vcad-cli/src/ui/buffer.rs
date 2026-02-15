//! Custom cell buffer for terminal UI rendering.
//!
//! Replaces ratatui with a minimal diff-based buffer that emits only changed cells.

use crossterm::{
    cursor::MoveTo,
    style::{Color as CtColor, SetBackgroundColor, SetForegroundColor},
    QueueableCommand,
};
use std::io::{self, Write};

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

/// 24-bit RGB color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// A single terminal cell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            fg: Color::rgb(0xF8, 0xF8, 0xF2),
            bg: Color::rgb(0x22, 0x22, 0x22),
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
            cell.fg = Color::rgb(0xF8, 0xF8, 0xF2);
            cell.bg = bg;
        }
    }

    /// Flush only changed cells to the terminal.
    pub fn flush(&mut self, stdout: &mut impl Write) -> io::Result<()> {
        let mut last_fg = Color::rgb(0, 0, 0);
        let mut last_bg = Color::rgb(0, 0, 0);
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
                    stdout.queue(SetForegroundColor(CtColor::Rgb {
                        r: cell.fg.r,
                        g: cell.fg.g,
                        b: cell.fg.b,
                    }))?;
                    last_fg = cell.fg;
                }
                if !colors_set || cell.bg != last_bg {
                    stdout.queue(SetBackgroundColor(CtColor::Rgb {
                        r: cell.bg.r,
                        g: cell.bg.g,
                        b: cell.bg.b,
                    }))?;
                    last_bg = cell.bg;
                }
                colors_set = true;

                // Write character
                write!(stdout, "{}", cell.ch)?;
                last_pos = Some((x, y));
            }
        }

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

/// Helper: set a character with colors at a position.
pub fn set_char(buf: &mut CellBuffer, x: u16, y: u16, ch: char, fg: Color, bg: Color) {
    if let Some(cell) = buf.cell_mut(x, y) {
        cell.ch = ch;
        cell.fg = fg;
        cell.bg = bg;
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
