//! Theme system — dark (Monokai Soda) and light palettes with auto-detection.

#![allow(dead_code)]

use std::sync::atomic::{AtomicBool, Ordering};

use super::buffer::Color;

/// Whether the active theme is dark. `true` = dark (default), `false` = light.
static DARK_MODE: AtomicBool = AtomicBool::new(true);

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

/// Detect terminal background from `COLORFGBG` env var.
/// Format is `fg;bg` — bg >= 8 is typically dark, < 8 is light.
/// Falls back to dark if the variable is missing or unparseable.
pub fn detect_dark_mode() -> bool {
    if let Ok(val) = std::env::var("COLORFGBG") {
        if let Some(bg_str) = val.rsplit(';').next() {
            if let Ok(bg) = bg_str.parse::<u8>() {
                return bg < 8;
            }
        }
    }
    true // default to dark
}

/// Initialize theme from environment. Call once at startup.
pub fn init() {
    DARK_MODE.store(detect_dark_mode(), Ordering::Relaxed);
}

/// Get the active theme.
pub fn active() -> &'static Theme {
    if DARK_MODE.load(Ordering::Relaxed) {
        &DARK
    } else {
        &LIGHT
    }
}

/// Returns true if currently in dark mode.
pub fn is_dark() -> bool {
    DARK_MODE.load(Ordering::Relaxed)
}

/// Toggle between dark and light themes. Returns the new mode name.
pub fn toggle() -> &'static str {
    let was_dark = DARK_MODE.load(Ordering::Relaxed);
    DARK_MODE.store(!was_dark, Ordering::Relaxed);
    if was_dark { "light" } else { "dark" }
}

// ── Convenience accessors (keep call sites concise) ──
// Named in UPPER_CASE to mirror the old constants they replaced.

#[allow(non_snake_case)]
pub fn BG() -> Color { active().bg }
#[allow(non_snake_case)]
pub fn SURFACE() -> Color { active().surface }
#[allow(non_snake_case)]
pub fn CARD() -> Color { active().card }
#[allow(non_snake_case)]
pub fn BORDER() -> Color { active().border }
#[allow(non_snake_case)]
pub fn TEXT() -> Color { active().text }
#[allow(non_snake_case)]
pub fn TEXT_MUTED() -> Color { active().text_muted }
#[allow(non_snake_case)]
pub fn ACCENT() -> Color { active().accent }
#[allow(non_snake_case)]
pub fn GREEN() -> Color { active().green }
#[allow(non_snake_case)]
pub fn YELLOW() -> Color { active().yellow }
#[allow(non_snake_case)]
pub fn ORANGE() -> Color { active().orange }
#[allow(non_snake_case)]
pub fn PURPLE() -> Color { active().purple }
#[allow(non_snake_case)]
pub fn CYAN() -> Color { active().cyan }
#[allow(non_snake_case)]
pub fn SELECTION_BG() -> Color { active().selection_bg }

/// Background clear color as RGB tuple for the rasterizer.
#[allow(non_snake_case)]
pub fn BG_RGB() -> (u8, u8, u8) { active().viewport_bg }

// Tab colors — these stay the same across themes (vibrant on both).
pub const TAB_CHAT: Color = Color::rgb(0xF9, 0x26, 0x72);
pub const TAB_CREATE: Color = Color::rgb(0x34, 0xD3, 0x99);
pub const TAB_TRANSFORM: Color = Color::rgb(0x60, 0xA5, 0xFA);
pub const TAB_COMBINE: Color = Color::rgb(0xA7, 0x8B, 0xFA);
pub const TAB_MODIFY: Color = Color::rgb(0xFB, 0xBF, 0x24);
pub const TAB_ASSEMBLY: Color = Color::rgb(0xFB, 0x71, 0x85);
pub const TAB_SIMULATE: Color = Color::rgb(0x22, 0xD3, 0xEE);
pub const TAB_EXPORT: Color = Color::rgb(0x94, 0xA3, 0xB8);

/// Get tab color by index (0-7).
pub fn tab_color(index: usize) -> Color {
    match index {
        0 => TAB_CHAT,
        1 => TAB_CREATE,
        2 => TAB_TRANSFORM,
        3 => TAB_COMBINE,
        4 => TAB_MODIFY,
        5 => TAB_ASSEMBLY,
        6 => TAB_SIMULATE,
        7 => TAB_EXPORT,
        _ => TEXT_MUTED(),
    }
}

/// Tab metadata: (icon, label, color).
pub const TABS: &[(&str, &str, Color)] = &[
    ("\u{2726}", "Chat", TAB_CHAT),       // ✦
    ("+", "Create", TAB_CREATE),           // +
    ("\u{2194}", "Xform", TAB_TRANSFORM),  // ↔
    ("\u{2295}", "Combine", TAB_COMBINE),  // ⊕
    ("\u{270E}", "Modify", TAB_MODIFY),    // ✎
    ("\u{2699}", "Assembly", TAB_ASSEMBLY), // ⚙
    ("\u{25B6}", "Simulate", TAB_SIMULATE), // ▶
    ("\u{2197}", "Export", TAB_EXPORT),     // ↗
];
