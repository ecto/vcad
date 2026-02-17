//! Render pixel buffers to any terminal.
//!
//! Supports multiple terminal graphics protocols:
//! - Kitty graphics (best quality)
//! - iTerm2 inline images
//! - Sixel graphics (wide compatibility)
//! - Unicode half-blocks (true color fallback)
//! - Braille characters (widest compatibility)

mod braille;
mod buffer;
mod output;
pub mod protocols;
mod rasterize;
mod terminal;

pub use braille::*;
pub use buffer::*;
pub use output::*;
pub use rasterize::*;
pub use terminal::*;
