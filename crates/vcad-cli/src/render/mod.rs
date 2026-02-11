//! Software 3D renderer for terminal display.
//!
//! Supports multiple terminal graphics protocols:
//! - Kitty graphics (best quality)
//! - iTerm2 inline images
//! - Sixel graphics (wide compatibility)
//! - Unicode half-blocks (true color fallback)
//! - Braille characters (widest compatibility)

mod output;
pub(crate) mod protocols;
mod rasterize;
mod sixel;
mod terminal;

pub use output::GraphicsOutput;
pub use rasterize::*;
pub use sixel::*;
