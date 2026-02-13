//! Monokai Soda theme — matching the vcad web app.

#![allow(dead_code)]

use super::buffer::Color;

// Core palette
pub const BG: Color = Color::rgb(0x22, 0x22, 0x22);
pub const SURFACE: Color = Color::rgb(0x2a, 0x2a, 0x2a);
pub const CARD: Color = Color::rgb(0x33, 0x33, 0x33);
pub const BORDER: Color = Color::rgb(0x44, 0x44, 0x44);
pub const TEXT: Color = Color::rgb(0xF8, 0xF8, 0xF2);
pub const TEXT_MUTED: Color = Color::rgb(0x75, 0x71, 0x5E);
pub const ACCENT: Color = Color::rgb(0xF9, 0x26, 0x72);

// Semantic
pub const GREEN: Color = Color::rgb(0xA6, 0xE2, 0x2E);
pub const YELLOW: Color = Color::rgb(0xE6, 0xDB, 0x74);
pub const ORANGE: Color = Color::rgb(0xFD, 0x97, 0x1F);
pub const PURPLE: Color = Color::rgb(0xAE, 0x81, 0xFF);
pub const CYAN: Color = Color::rgb(0x66, 0xD9, 0xEF);

// Selection highlight background (accent at ~20% opacity on SURFACE)
pub const SELECTION_BG: Color = Color::rgb(0x3a, 0x1a, 0x2a);

// Tab colors (matching web toolbar)
pub const TAB_CHAT: Color = ACCENT;
pub const TAB_CREATE: Color = Color::rgb(0x34, 0xD3, 0x99);
pub const TAB_TRANSFORM: Color = Color::rgb(0x60, 0xA5, 0xFA);
pub const TAB_COMBINE: Color = Color::rgb(0xA7, 0x8B, 0xFA);
pub const TAB_MODIFY: Color = Color::rgb(0xFB, 0xBF, 0x24);
pub const TAB_ASSEMBLY: Color = Color::rgb(0xFB, 0x71, 0x85);
pub const TAB_SIMULATE: Color = Color::rgb(0x22, 0xD3, 0xEE);
pub const TAB_EXPORT: Color = Color::rgb(0x94, 0xA3, 0xB8);

/// Background clear color as RGB tuple for the rasterizer.
pub const BG_RGB: (u8, u8, u8) = (0x22, 0x22, 0x22);

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
        _ => TEXT_MUTED,
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
