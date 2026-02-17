//! Software 3D renderer for terminal display.
//!
//! Re-exports from the `termview` crate. Supports multiple terminal graphics protocols:
//! - Kitty graphics (best quality)
//! - iTerm2 inline images
//! - Sixel graphics (wide compatibility)
//! - Unicode half-blocks (true color fallback)
//! - Braille characters (widest compatibility)

pub use termview::*;
