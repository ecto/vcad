//! Terminal graphics capability detection.
//!
//! Detects the best available graphics protocol for the current terminal.

#![allow(dead_code)] // These will be used in TUI mode

use std::env;

/// Detected terminal graphics capability
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphicsProtocol {
    /// Kitty graphics protocol - best quality, true color, partial updates
    Kitty,
    /// Sixel graphics - DEC standard, wide support (xterm, mlterm, foot)
    Sixel,
    /// iTerm2 inline images - macOS native
    ITerm2,
    /// Unicode half-blocks (upper/lower) with true color
    HalfBlock,
    /// Braille characters (2x4 dot patterns) - widest compatibility
    Braille,
}

/// Terminal capabilities
#[derive(Debug, Clone)]
pub struct TerminalCaps {
    /// Best available graphics protocol
    pub protocol: GraphicsProtocol,
    /// Whether true color (24-bit) is supported
    pub true_color: bool,
    /// Terminal width in pixels (if detectable)
    pub width_px: Option<u32>,
    /// Terminal height in pixels (if detectable)
    pub height_px: Option<u32>,
    /// Approximate pixels per cell width
    pub cell_width: u32,
    /// Approximate pixels per cell height
    pub cell_height: u32,
}

impl Default for TerminalCaps {
    fn default() -> Self {
        Self::detect()
    }
}

impl TerminalCaps {
    /// Detect terminal graphics capabilities.
    ///
    /// Priority order: Kitty > iTerm2 > Sixel > HalfBlock > Braille
    pub fn detect() -> Self {
        // 1. Check for Kitty
        if env::var("KITTY_WINDOW_ID").is_ok() {
            return Self::kitty();
        }

        // 2. Check for WezTerm (supports Kitty protocol)
        if env::var("WEZTERM_PANE").is_ok() {
            return Self::kitty();
        }

        // 3. Check for iTerm2
        if env::var("TERM_PROGRAM")
            .map(|v| v == "iTerm.app")
            .unwrap_or(false)
        {
            return Self::iterm2();
        }

        // Also check LC_TERMINAL for nested sessions
        if env::var("LC_TERMINAL")
            .map(|v| v == "iTerm2")
            .unwrap_or(false)
        {
            return Self::iterm2();
        }

        // 4. Check for terminals with known Sixel support
        if Self::likely_sixel_support() {
            return Self::sixel();
        }

        // 5. Check COLORTERM for true color support
        let true_color = env::var("COLORTERM")
            .map(|v| v == "truecolor" || v == "24bit")
            .unwrap_or(false);

        // Use half-block if we have true color, braille otherwise
        Self {
            protocol: if true_color {
                GraphicsProtocol::HalfBlock
            } else {
                GraphicsProtocol::Braille
            },
            true_color,
            width_px: None,
            height_px: None,
            cell_width: 8,
            cell_height: 16,
        }
    }

    /// Check if terminal likely supports Sixel based on TERM variable.
    fn likely_sixel_support() -> bool {
        if let Ok(term) = env::var("TERM") {
            // These terminals commonly support Sixel
            let sixel_terms = [
                "xterm-256color",
                "xterm",
                "mlterm",
                "foot",
                "foot-extra",
                "contour",
                "yaft-256color",
            ];

            for supported in sixel_terms {
                if term.starts_with(supported) {
                    return true;
                }
            }
        }

        // Check for explicit SIXEL environment hint
        if env::var("TERM_SIXEL").is_ok() {
            return true;
        }

        false
    }

    fn kitty() -> Self {
        Self {
            protocol: GraphicsProtocol::Kitty,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 10,
            cell_height: 20,
        }
    }

    fn iterm2() -> Self {
        Self {
            protocol: GraphicsProtocol::ITerm2,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 9,
            cell_height: 18,
        }
    }

    fn sixel() -> Self {
        Self {
            protocol: GraphicsProtocol::Sixel,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 8,
            cell_height: 16,
        }
    }

    /// Get the resolution multiplier for braille rendering.
    ///
    /// Returns (width_mult, height_mult) - how many "pixels" per character cell.
    pub fn braille_resolution(&self) -> (u32, u32) {
        // Braille uses 2x4 dot pattern
        (2, 4)
    }

    /// Get the resolution multiplier for half-block rendering.
    ///
    /// Returns (width_mult, height_mult) - how many "pixels" per character cell.
    pub fn halfblock_resolution(&self) -> (u32, u32) {
        // Half-block uses 1x2 (upper half / lower half)
        (1, 2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_defaults() {
        // Just verify detection doesn't panic
        let caps = TerminalCaps::detect();
        assert!(caps.cell_width > 0);
        assert!(caps.cell_height > 0);
    }

    #[test]
    fn test_braille_resolution() {
        let caps = TerminalCaps::default();
        let (w, h) = caps.braille_resolution();
        assert_eq!(w, 2);
        assert_eq!(h, 4);
    }
}
