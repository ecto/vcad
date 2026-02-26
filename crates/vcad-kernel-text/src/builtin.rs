//! Built-in font data.
//!
//! Contains an embedded subset of Open Sans Regular for basic text rendering.
//! The font is embedded at compile time for standalone operation.

/// Open Sans Regular font data (embedded at compile time).
///
/// This is the Noto Sans font from the Vercel OG package, which is similar to Open Sans
/// and freely licensed. For production use, you may want to replace this with a custom
/// font or download Open Sans directly.
///
/// Note: The actual font bytes are included via include_bytes!() at compile time.
/// The path is relative to this source file.
#[cfg(not(feature = "no-builtin-font"))]
pub static OPEN_SANS_REGULAR: &[u8] = include_bytes!(
    "../../../node_modules/next/dist/compiled/@vercel/og/noto-sans-v27-latin-regular.ttf"
);

/// Fallback for when no builtin font is available.
#[cfg(feature = "no-builtin-font")]
pub static OPEN_SANS_REGULAR: &[u8] = &[];
