//! Built-in font data.
//!
//! Contains an embedded copy of Noto Sans Regular for basic text rendering.
//! The font is vendored in `assets/` and embedded at compile time, so the crate
//! builds standalone (no `node_modules`, no network, no build script).

/// Noto Sans Regular font data (embedded at compile time).
///
/// Vendored at `assets/NotoSans-Regular.ttf` (Noto Sans Regular v2.007,
/// Copyright 2015-2021 Google LLC), licensed under the SIL Open Font License
/// 1.1 — see `assets/LICENSE-NotoSans.txt`, which redistribution requires be
/// kept alongside the font.
///
/// Disable with the `no-builtin-font` feature to drop the ~27 KB from the binary.
#[cfg(not(feature = "no-builtin-font"))]
pub static OPEN_SANS_REGULAR: &[u8] = include_bytes!("../assets/NotoSans-Regular.ttf");

/// Fallback for when no builtin font is available.
#[cfg(feature = "no-builtin-font")]
pub static OPEN_SANS_REGULAR: &[u8] = &[];
