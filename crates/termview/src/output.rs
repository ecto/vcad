//! Unified graphics output manager.
//!
//! Provides a single interface for rendering to any supported terminal graphics protocol.

#![allow(dead_code)]

use std::io::{self, Write};

use crate::protocols::{kitty, sixel};
use crate::terminal::{GraphicsProtocol, TerminalCaps};
use crate::{buffer_to_braille, RenderBuffer};

/// Graphics output manager that automatically uses the best available protocol.
pub struct GraphicsOutput {
    caps: TerminalCaps,
    image_id: u32,
    /// Whether running inside tmux (for passthrough wrapping).
    in_tmux: bool,
}

impl Default for GraphicsOutput {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphicsOutput {
    /// Create a new graphics output manager with auto-detected capabilities.
    pub fn new() -> Self {
        let caps = TerminalCaps::detect();
        let in_tmux = caps.in_tmux;
        Self {
            caps,
            image_id: 1,
            in_tmux,
        }
    }

    /// Create a graphics output manager with explicit capabilities.
    pub fn with_caps(caps: TerminalCaps) -> Self {
        let in_tmux = caps.in_tmux;
        Self {
            caps,
            image_id: 1,
            in_tmux,
        }
    }

    /// Get the detected graphics protocol.
    pub fn protocol(&self) -> GraphicsProtocol {
        self.caps.protocol
    }

    /// Get the terminal capabilities.
    pub fn caps(&self) -> &TerminalCaps {
        &self.caps
    }

    /// Render buffer to terminal at current cursor position.
    pub fn display(&mut self, buffer: &RenderBuffer, stdout: &mut impl Write) -> io::Result<()> {
        match self.caps.protocol {
            GraphicsProtocol::Kitty => {
                let img = kitty::render_buffer_to_kitty(buffer, self.image_id);
                self.image_id = self.image_id.wrapping_add(1);
                if self.image_id == 0 {
                    self.image_id = 1;
                }
                img.display(stdout, self.in_tmux)
            }
            GraphicsProtocol::ITerm2 => self.display_iterm2(buffer, stdout),
            GraphicsProtocol::Sixel => {
                let mut encoder = sixel::SixelEncoder::new(buffer.width, buffer.height);
                let sixel_data = encoder.encode(&buffer.pixels);
                write!(stdout, "{}", sixel_data)
            }
            GraphicsProtocol::HalfBlock => self.display_halfblock(buffer, stdout),
            GraphicsProtocol::Braille => {
                let (_, _, braille) = buffer_to_braille(buffer);
                write!(stdout, "{}", braille)
            }
        }
    }

    /// Display image using iTerm2 inline image protocol.
    fn display_iterm2(&self, buffer: &RenderBuffer, stdout: &mut impl Write) -> io::Result<()> {
        use base64::{engine::general_purpose::STANDARD, Engine};

        // Encode as PNG for smaller size
        let png_data = self.encode_png(buffer)?;
        let b64 = STANDARD.encode(&png_data);

        // iTerm2 inline images: OSC 1337 ; File=inline=1:BASE64 ST
        write!(
            stdout,
            "\x1b]1337;File=inline=1;width=auto;height=auto:{}\x07",
            b64
        )
    }

    /// Display using Unicode half-block characters.
    ///
    /// Each character cell represents 2 vertical pixels using the upper half block character.
    fn display_halfblock(&self, buffer: &RenderBuffer, stdout: &mut impl Write) -> io::Result<()> {
        // Use upper half block (upper half filled, lower half background)
        // Set foreground to top pixel color, background to bottom pixel color

        for y in (0..buffer.height).step_by(2) {
            for x in 0..buffer.width {
                let top_idx = (y * buffer.width + x) as usize * 4;

                let (tr, tg, tb) = if top_idx + 2 < buffer.pixels.len() {
                    (
                        buffer.pixels[top_idx],
                        buffer.pixels[top_idx + 1],
                        buffer.pixels[top_idx + 2],
                    )
                } else {
                    (0, 0, 0)
                };

                let (br, bg, bb) = if y + 1 < buffer.height {
                    let bot_idx = ((y + 1) * buffer.width + x) as usize * 4;
                    if bot_idx + 2 < buffer.pixels.len() {
                        (
                            buffer.pixels[bot_idx],
                            buffer.pixels[bot_idx + 1],
                            buffer.pixels[bot_idx + 2],
                        )
                    } else {
                        (0, 0, 0)
                    }
                } else {
                    (0, 0, 0)
                };

                // ANSI: foreground (top), background (bottom), upper half block
                write!(
                    stdout,
                    "\x1b[38;2;{};{};{};48;2;{};{};{}m\u{2580}",
                    tr, tg, tb, br, bg, bb
                )?;
            }
            writeln!(stdout, "\x1b[0m")?;
        }

        Ok(())
    }

    /// Encode render buffer as PNG bytes.
    fn encode_png(&self, buffer: &RenderBuffer) -> io::Result<Vec<u8>> {
        use image::{ImageBuffer, ImageEncoder, Rgba};

        let img: ImageBuffer<Rgba<u8>, _> =
            ImageBuffer::from_raw(buffer.width, buffer.height, buffer.pixels.clone()).ok_or_else(
                || io::Error::new(io::ErrorKind::InvalidData, "buffer size mismatch"),
            )?;

        let mut png_bytes = Vec::new();
        let encoder = image::codecs::png::PngEncoder::new(&mut png_bytes);

        encoder
            .write_image(
                img.as_raw(),
                buffer.width,
                buffer.height,
                image::ExtendedColorType::Rgba8,
            )
            .map_err(io::Error::other)?;

        Ok(png_bytes)
    }

    /// Render buffer to a file as PNG.
    pub fn save_png(&self, buffer: &RenderBuffer, path: &std::path::Path) -> io::Result<()> {
        let png_data = self.encode_png(buffer)?;
        std::fs::write(path, png_data)
    }
}

/// Convert render buffer to a string for the given protocol.
///
/// Returns None for protocols that require direct terminal output (Kitty, iTerm2).
pub fn buffer_to_string(buffer: &RenderBuffer, protocol: GraphicsProtocol) -> Option<String> {
    match protocol {
        GraphicsProtocol::Kitty | GraphicsProtocol::ITerm2 => {
            // These require escape sequences written directly
            None
        }
        GraphicsProtocol::Sixel => {
            let mut encoder = sixel::SixelEncoder::new(buffer.width, buffer.height);
            Some(encoder.encode(&buffer.pixels))
        }
        GraphicsProtocol::HalfBlock => Some(buffer_to_halfblock(buffer)),
        GraphicsProtocol::Braille => {
            let (_, _, s) = buffer_to_braille(buffer);
            Some(s)
        }
    }
}

/// Convert render buffer to half-block string.
fn buffer_to_halfblock(buffer: &RenderBuffer) -> String {
    let mut result = String::new();

    for y in (0..buffer.height).step_by(2) {
        for x in 0..buffer.width {
            let top_idx = (y * buffer.width + x) as usize * 4;

            let (tr, tg, tb) = if top_idx + 2 < buffer.pixels.len() {
                (
                    buffer.pixels[top_idx],
                    buffer.pixels[top_idx + 1],
                    buffer.pixels[top_idx + 2],
                )
            } else {
                (0, 0, 0)
            };

            let (br, bg, bb) = if y + 1 < buffer.height {
                let bot_idx = ((y + 1) * buffer.width + x) as usize * 4;
                if bot_idx + 2 < buffer.pixels.len() {
                    (
                        buffer.pixels[bot_idx],
                        buffer.pixels[bot_idx + 1],
                        buffer.pixels[bot_idx + 2],
                    )
                } else {
                    (0, 0, 0)
                }
            } else {
                (0, 0, 0)
            };

            result.push_str(&format!(
                "\x1b[38;2;{};{};{};48;2;{};{};{}m\u{2580}",
                tr, tg, tb, br, bg, bb
            ));
        }
        result.push_str("\x1b[0m\n");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_graphics_output_creation() {
        let output = GraphicsOutput::new();
        // Just verify it doesn't panic
        assert!(output.image_id > 0);
    }

    #[test]
    fn test_halfblock_conversion() {
        let mut buffer = RenderBuffer::new(4, 4);
        buffer.clear(100, 100, 100);
        let result = buffer_to_halfblock(&buffer);
        assert!(!result.is_empty());
        assert!(result.contains('\u{2580}')); // Upper half block
    }

    #[test]
    fn test_buffer_to_string_braille() {
        let mut buffer = RenderBuffer::new(4, 8);
        buffer.clear(200, 200, 200);
        let result = buffer_to_string(&buffer, GraphicsProtocol::Braille);
        assert!(result.is_some());
    }
}
