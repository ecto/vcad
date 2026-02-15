//! Kitty graphics protocol implementation.
//!
//! The Kitty terminal graphics protocol allows for high-quality image display
//! with true color support, partial updates, and Unicode placeholders.
//!
//! See: <https://sw.kovidgoyal.net/kitty/graphics-protocol/>

use base64::{engine::general_purpose::STANDARD, Engine};
use std::io::{self, Write};

use crate::render::RenderBuffer;

/// Kitty graphics image wrapper.
pub struct KittyImage {
    /// Unique image ID for the terminal
    pub id: u32,
    /// Image width in pixels
    pub width: u32,
    /// Image height in pixels
    pub height: u32,
    /// RGBA pixel data
    pub data: Vec<u8>,
}

impl KittyImage {
    /// Create a new KittyImage from dimensions and RGBA pixel data.
    pub fn new(id: u32, width: u32, height: u32, data: Vec<u8>) -> Self {
        Self {
            id,
            width,
            height,
            data,
        }
    }

    /// Encode and display image at cursor position.
    ///
    /// Transmits the image data to the terminal using the Kitty graphics protocol.
    /// When `in_tmux` is true, wraps each escape sequence in DCS passthrough
    /// (requires `set -g allow-passthrough on` in tmux.conf).
    pub fn display(&self, stdout: &mut impl Write, in_tmux: bool) -> io::Result<()> {
        // Kitty protocol: ESC_G<payload>ESC\
        // a=T (transmit), f=32 (RGBA), s=width, v=height, i=id

        let b64 = STANDARD.encode(&self.data);
        let chunk_size = 4096;

        let chunks: Vec<&[u8]> = b64.as_bytes().chunks(chunk_size).collect();

        for (i, chunk) in chunks.iter().enumerate() {
            let more = if i < chunks.len() - 1 { "m=1" } else { "m=0" };
            let chunk_str = std::str::from_utf8(chunk).map_err(|e| {
                io::Error::new(io::ErrorKind::InvalidData, e)
            })?;

            let seq = if i == 0 {
                // First chunk: include all parameters
                format!(
                    "\x1b_Ga=T,f=32,s={},v={},i={},{};{}\x1b\\",
                    self.width, self.height, self.id, more, chunk_str
                )
            } else {
                // Continuation chunks
                format!("\x1b_G{};{}\x1b\\", more, chunk_str)
            };

            if in_tmux {
                write!(stdout, "{}", tmux_wrap(&seq))?;
            } else {
                write!(stdout, "{}", seq)?;
            }
        }

        stdout.flush()
    }

    /// Display image at specific cell position using Unicode placeholders.
    ///
    /// This allows the image to scroll with text content.
    #[allow(dead_code)]
    pub fn display_at(
        &self,
        stdout: &mut impl Write,
        _col: u16,
        _row: u16,
        cols: u16,
        rows: u16,
        in_tmux: bool,
    ) -> io::Result<()> {
        // First transmit the image data
        self.display(stdout, in_tmux)?;

        // Then place it using Unicode placeholders
        write!(
            stdout,
            "\x1b_Ga=p,i={},p=1,q=2,c={},r={}\x1b\\",
            self.id, cols, rows
        )?;

        stdout.flush()
    }

    /// Delete image from terminal memory.
    #[allow(dead_code)]
    pub fn delete(&self, stdout: &mut impl Write) -> io::Result<()> {
        write!(stdout, "\x1b_Ga=d,d=i,i={}\x1b\\", self.id)?;
        stdout.flush()
    }
}

/// Wrap an escape sequence in tmux DCS passthrough.
///
/// Doubles all ESC bytes inside the payload and wraps with `\x1bPtmux;...\x1b\\`.
/// Requires `set -g allow-passthrough on` in tmux.conf.
fn tmux_wrap(seq: &str) -> String {
    let mut out = String::with_capacity(seq.len() + 16);
    out.push_str("\x1bPtmux;");
    for ch in seq.chars() {
        if ch == '\x1b' {
            out.push_str("\x1b\x1b");
        } else {
            out.push(ch);
        }
    }
    out.push_str("\x1b\\");
    out
}

/// Create KittyImage from RenderBuffer.
pub fn render_buffer_to_kitty(buffer: &RenderBuffer, id: u32) -> KittyImage {
    KittyImage {
        id,
        width: buffer.width,
        height: buffer.height,
        data: buffer.pixels.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kitty_image_creation() {
        let data = vec![255u8; 4 * 10 * 10]; // 10x10 white image
        let img = KittyImage::new(1, 10, 10, data);
        assert_eq!(img.width, 10);
        assert_eq!(img.height, 10);
        assert_eq!(img.data.len(), 400);
    }

    #[test]
    fn test_display_to_buffer() {
        let data = vec![255u8; 4 * 2 * 2]; // 2x2 image
        let img = KittyImage::new(1, 2, 2, data);

        let mut output = Vec::new();
        img.display(&mut output, false).unwrap();

        // Check that it starts with the Kitty escape sequence
        assert!(output.starts_with(b"\x1b_G"));
        // Check that it ends with the terminator
        assert!(output.ends_with(b"\x1b\\"));
    }
}
