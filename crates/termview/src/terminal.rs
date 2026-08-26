//! Terminal graphics capability detection.
//!
//! Detects the best available graphics protocol for the current terminal.

#![allow(dead_code)]

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
    /// Whether running inside tmux (for passthrough wrapping)
    pub in_tmux: bool,
}

impl Default for TerminalCaps {
    fn default() -> Self {
        Self::detect()
    }
}

impl TerminalCaps {
    /// Detect terminal graphics capabilities.
    ///
    /// Priority order: TERMVIEW_PROTOCOL/VCAD_PROTOCOL override > Kitty > iTerm2 > Sixel > HalfBlock > Braille
    pub fn detect() -> Self {
        let in_tmux = env::var("TMUX").is_ok();

        // 0. Explicit override via TERMVIEW_PROTOCOL env var (fall back to VCAD_PROTOCOL)
        let proto_override = env::var("TERMVIEW_PROTOCOL")
            .or_else(|_| env::var("VCAD_PROTOCOL"))
            .ok();

        if let Some(proto) = proto_override {
            let protocol = match proto.to_lowercase().as_str() {
                "kitty" => GraphicsProtocol::Kitty,
                "sixel" => GraphicsProtocol::Sixel,
                "iterm2" => GraphicsProtocol::ITerm2,
                "halfblock" => GraphicsProtocol::HalfBlock,
                "braille" => GraphicsProtocol::Braille,
                _ => GraphicsProtocol::HalfBlock,
            };
            let (cell_width, cell_height) = match protocol {
                GraphicsProtocol::Kitty => (10, 20),
                GraphicsProtocol::ITerm2 => (9, 18),
                GraphicsProtocol::Sixel => (8, 16),
                _ => (8, 16),
            };
            return Self {
                protocol,
                true_color: true,
                width_px: None,
                height_px: None,
                cell_width,
                cell_height,
                in_tmux,
            };
        }

        // Pixel protocols (Kitty/iTerm2/Sixel) only work when NOT inside tmux,
        // because tmux intercepts escape sequences. Users who have configured
        // `set -g allow-passthrough on` can use TERMVIEW_PROTOCOL=kitty to override.
        if !in_tmux {
            // 1. Check for Kitty
            if env::var("KITTY_WINDOW_ID").is_ok() {
                return Self::kitty_with_tmux(false);
            }

            // 2. Check for WezTerm (supports Kitty protocol)
            if env::var("WEZTERM_PANE").is_ok() {
                return Self::kitty_with_tmux(false);
            }

            // 3. Check for Ghostty (supports Kitty protocol)
            if env::var("GHOSTTY_BIN_DIR").is_ok() {
                return Self::kitty_with_tmux(false);
            }

            // 4. Check for iTerm2
            if env::var("TERM_PROGRAM")
                .map(|v| v == "iTerm.app")
                .unwrap_or(false)
            {
                return Self::iterm2_with_tmux(false);
            }

            // Also check LC_TERMINAL for nested sessions
            if env::var("LC_TERMINAL")
                .map(|v| v == "iTerm2")
                .unwrap_or(false)
            {
                return Self::iterm2_with_tmux(false);
            }

            // 5. Check for terminals with known Sixel support
            if Self::likely_sixel_support() {
                return Self::sixel_with_tmux(false);
            }
        }

        // 6. Check COLORTERM for true color support
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
            in_tmux,
        }
    }

    /// Check if terminal likely supports Sixel based on TERM variable.
    fn likely_sixel_support() -> bool {
        if let Ok(term) = env::var("TERM") {
            // Only terminals with reliable Sixel support
            let sixel_terms = ["mlterm", "foot", "foot-extra", "contour", "yaft-256color"];

            for supported in sixel_terms {
                if term.starts_with(supported) {
                    return true;
                }
            }
        }

        // Check for explicit SIXEL environment hint
        env::var("TERM_SIXEL").is_ok()
    }

    fn kitty_with_tmux(in_tmux: bool) -> Self {
        Self {
            protocol: GraphicsProtocol::Kitty,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 10,
            cell_height: 20,
            in_tmux,
        }
    }

    fn iterm2_with_tmux(in_tmux: bool) -> Self {
        Self {
            protocol: GraphicsProtocol::ITerm2,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 9,
            cell_height: 18,
            in_tmux,
        }
    }

    fn sixel_with_tmux(in_tmux: bool) -> Self {
        Self {
            protocol: GraphicsProtocol::Sixel,
            true_color: true,
            width_px: None,
            height_px: None,
            cell_width: 8,
            cell_height: 16,
            in_tmux,
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
