//! Theme system — Terminal (user palette), Dark (Monokai Soda), Light.
//!
//! Terminal is the default: all chrome colors resolve to `Color::Default`
//! (SGR 39/49, the terminal's current fg/bg) and semantic accents resolve
//! to the 16 ANSI named colors so they inherit whatever palette the user's
//! terminal already has. Dark and Light remain hardcoded RGB themes for
//! users who want the full Monokai Soda / paper-cream look regardless of
//! terminal config.

#![allow(dead_code)]

use std::sync::atomic::{AtomicU8, Ordering};

use super::buffer::{Color, NamedColor};

/// Active theme mode. Stored as a `u8` so we can swap it atomically.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeMode {
    Terminal = 0,
    Dark = 1,
    Light = 2,
}

impl ThemeMode {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => ThemeMode::Dark,
            2 => ThemeMode::Light,
            _ => ThemeMode::Terminal,
        }
    }
    fn name(self) -> &'static str {
        match self {
            ThemeMode::Terminal => "terminal",
            ThemeMode::Dark => "dark",
            ThemeMode::Light => "light",
        }
    }
}

/// Active theme mode. Starts Terminal so first-launch users see their
/// own palette instead of Monokai.
static THEME_MODE: AtomicU8 = AtomicU8::new(ThemeMode::Terminal as u8);

/// A complete UI color palette.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    // Core palette
    pub bg: Color,
    pub surface: Color,
    pub card: Color,
    pub border: Color,
    pub text: Color,
    pub text_muted: Color,
    pub accent: Color,
    // Semantic
    pub green: Color,
    pub yellow: Color,
    pub orange: Color,
    pub purple: Color,
    pub cyan: Color,
    // Selection
    pub selection_bg: Color,
    // Viewport
    pub viewport_bg: (u8, u8, u8),
}

/// Dark theme — Monokai Soda (original).
pub const DARK: Theme = Theme {
    bg: Color::rgb(0x22, 0x22, 0x22),
    surface: Color::rgb(0x2a, 0x2a, 0x2a),
    card: Color::rgb(0x33, 0x33, 0x33),
    border: Color::rgb(0x44, 0x44, 0x44),
    text: Color::rgb(0xF8, 0xF8, 0xF2),
    text_muted: Color::rgb(0x75, 0x71, 0x5E),
    accent: Color::rgb(0xF9, 0x26, 0x72),
    green: Color::rgb(0xA6, 0xE2, 0x2E),
    yellow: Color::rgb(0xE6, 0xDB, 0x74),
    orange: Color::rgb(0xFD, 0x97, 0x1F),
    purple: Color::rgb(0xAE, 0x81, 0xFF),
    cyan: Color::rgb(0x66, 0xD9, 0xEF),
    selection_bg: Color::rgb(0x3a, 0x1a, 0x2a),
    viewport_bg: (0x22, 0x22, 0x22),
};

/// Light theme — high-contrast for light terminal backgrounds.
pub const LIGHT: Theme = Theme {
    bg: Color::rgb(0xF5, 0xF5, 0xF0),
    surface: Color::rgb(0xEB, 0xEB, 0xE4),
    card: Color::rgb(0xDE, 0xDE, 0xD6),
    border: Color::rgb(0xBB, 0xBB, 0xB0),
    text: Color::rgb(0x1E, 0x1E, 0x1E),
    text_muted: Color::rgb(0x6B, 0x6B, 0x60),
    accent: Color::rgb(0xD1, 0x1A, 0x5C),
    green: Color::rgb(0x3A, 0x8C, 0x0A),
    yellow: Color::rgb(0x9C, 0x8A, 0x00),
    orange: Color::rgb(0xC4, 0x6C, 0x00),
    purple: Color::rgb(0x6C, 0x47, 0xCC),
    cyan: Color::rgb(0x0E, 0x7C, 0x9E),
    selection_bg: Color::rgb(0xE0, 0xCF, 0xD6),
    viewport_bg: (0xE8, 0xE8, 0xE0),
};

/// Terminal theme — inherits the user's own palette. `bg/surface/card/
/// text` all resolve to `Color::Default` so the TUI doesn't clobber the
/// terminal's current fg/bg, and the semantic accents map to the 16 ANSI
/// named colors so the user's existing colorscheme decides exactly how
/// each one renders. `viewport_bg` stays a concrete dark RGB since the
/// rasterizer needs a real clear value and the terminal doesn't expose
/// its own bg to the process.
pub const TERMINAL: Theme = Theme {
    bg: Color::Default,
    surface: Color::Default,
    card: Color::Default,
    border: Color::Named(NamedColor::DarkGrey),
    text: Color::Default,
    text_muted: Color::Named(NamedColor::DarkGrey),
    accent: Color::Named(NamedColor::Red),
    green: Color::Named(NamedColor::Green),
    yellow: Color::Named(NamedColor::Yellow),
    orange: Color::Named(NamedColor::DarkYellow),
    purple: Color::Named(NamedColor::Magenta),
    cyan: Color::Named(NamedColor::Cyan),
    selection_bg: Color::Named(NamedColor::DarkBlue),
    viewport_bg: (0x11, 0x11, 0x11),
};

/// Initialize theme from environment. Call once at startup. For now
/// this is a no-op — `THEME_MODE` already defaults to Terminal and we
/// no longer auto-detect from `COLORFGBG` since the whole point of the
/// Terminal theme is that the user's palette decides.
pub fn init() {}

/// Get the active theme.
pub fn active() -> &'static Theme {
    match ThemeMode::from_u8(THEME_MODE.load(Ordering::Relaxed)) {
        ThemeMode::Terminal => &TERMINAL,
        ThemeMode::Dark => &DARK,
        ThemeMode::Light => &LIGHT,
    }
}

/// Current theme mode.
pub fn mode() -> ThemeMode {
    ThemeMode::from_u8(THEME_MODE.load(Ordering::Relaxed))
}

/// Returns true if currently in the Dark theme. Callers that used to
/// gate on "dark vs light" should prefer `mode()` directly now that
/// there are three options.
pub fn is_dark() -> bool {
    matches!(mode(), ThemeMode::Dark)
}

/// Cycle through Terminal → Dark → Light → Terminal. Returns the new
/// mode name for the status bar.
pub fn toggle() -> &'static str {
    let current = mode();
    let next = match current {
        ThemeMode::Terminal => ThemeMode::Dark,
        ThemeMode::Dark => ThemeMode::Light,
        ThemeMode::Light => ThemeMode::Terminal,
    };
    THEME_MODE.store(next as u8, Ordering::Relaxed);
    next.name()
}

// ── Convenience accessors (keep call sites concise) ──
// Named in UPPER_CASE to mirror the old constants they replaced.

#[allow(non_snake_case)]
pub fn BG() -> Color {
    active().bg
}
#[allow(non_snake_case)]
pub fn SURFACE() -> Color {
    active().surface
}
#[allow(non_snake_case)]
pub fn CARD() -> Color {
    active().card
}
#[allow(non_snake_case)]
pub fn BORDER() -> Color {
    active().border
}
#[allow(non_snake_case)]
pub fn TEXT() -> Color {
    active().text
}
#[allow(non_snake_case)]
pub fn TEXT_MUTED() -> Color {
    active().text_muted
}
#[allow(non_snake_case)]
pub fn ACCENT() -> Color {
    active().accent
}
#[allow(non_snake_case)]
pub fn GREEN() -> Color {
    active().green
}
#[allow(non_snake_case)]
pub fn YELLOW() -> Color {
    active().yellow
}
#[allow(non_snake_case)]
pub fn ORANGE() -> Color {
    active().orange
}
#[allow(non_snake_case)]
pub fn PURPLE() -> Color {
    active().purple
}
#[allow(non_snake_case)]
pub fn CYAN() -> Color {
    active().cyan
}
#[allow(non_snake_case)]
pub fn SELECTION_BG() -> Color {
    active().selection_bg
}

/// Background clear color as RGB tuple for the rasterizer.
#[allow(non_snake_case)]
pub fn BG_RGB() -> (u8, u8, u8) {
    active().viewport_bg
}

// Tab colors — these stay the same across themes (vibrant on both).
pub const TAB_CREATE: Color = Color::rgb(0x34, 0xD3, 0x99);
pub const TAB_TRANSFORM: Color = Color::rgb(0x60, 0xA5, 0xFA);
pub const TAB_COMBINE: Color = Color::rgb(0xA7, 0x8B, 0xFA);
pub const TAB_MODIFY: Color = Color::rgb(0xFB, 0xBF, 0x24);
pub const TAB_ASSEMBLY: Color = Color::rgb(0xFB, 0x71, 0x85);
pub const TAB_SIMULATE: Color = Color::rgb(0x22, 0xD3, 0xEE);
pub const TAB_EXPORT: Color = Color::rgb(0x94, 0xA3, 0xB8);

/// Get tab color by index (0-6). Matches the `TABS` array ordering.
pub fn tab_color(index: usize) -> Color {
    match index {
        0 => TAB_CREATE,
        1 => TAB_TRANSFORM,
        2 => TAB_COMBINE,
        3 => TAB_MODIFY,
        4 => TAB_ASSEMBLY,
        5 => TAB_SIMULATE,
        6 => TAB_EXPORT,
        _ => TEXT_MUTED(),
    }
}

/// Tab metadata: (icon, label, color). Mirrors the web app's `ALL_TABS`
/// ordering in `ToolPalette.tsx` — chat lives in its own sidebar, not in the
/// tab strip.
pub const TABS: &[(&str, &str, Color)] = &[
    ("+", "Create", TAB_CREATE),              // +
    ("\u{2194}", "Transform", TAB_TRANSFORM), // ↔
    ("\u{2295}", "Combine", TAB_COMBINE),     // ⊕
    ("\u{270E}", "Modify", TAB_MODIFY),       // ✎
    ("\u{2699}", "Assembly", TAB_ASSEMBLY),   // ⚙
    ("\u{25B6}", "Simulate", TAB_SIMULATE),   // ▶
    ("\u{2197}", "Export", TAB_EXPORT),       // ↗
];
