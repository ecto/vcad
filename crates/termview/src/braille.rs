//! Braille character rendering.
//!
//! Converts pixel buffers to braille character art.

use crate::buffer::RenderBuffer;

/// Convert a render buffer to braille character art.
///
/// Each braille character represents a 2x4 pixel grid.
/// Returns (width_chars, height_chars, string).
pub fn buffer_to_braille(buffer: &RenderBuffer) -> (u16, u16, String) {
    let char_width = (buffer.width / 2) as u16;
    let char_height = (buffer.height / 4) as u16;

    let mut result = String::with_capacity((char_width as usize + 1) * char_height as usize);

    for cy in 0..char_height {
        for cx in 0..char_width {
            let px = (cx * 2) as u32;
            let py = (cy * 4) as u32;

            // Sample 2x4 pixels
            let mut dots = 0u8;
            let mut total_r = 0u32;
            let mut total_g = 0u32;
            let mut total_b = 0u32;
            let mut count = 0u32;

            // Braille dot pattern:
            // 0 3
            // 1 4
            // 2 5
            // 6 7
            for dy in 0..4 {
                for dx in 0..2 {
                    let x = px + dx;
                    let y = py + dy;
                    if x < buffer.width && y < buffer.height {
                        let idx = (y * buffer.width + x) as usize * 4;
                        let r = buffer.pixels[idx] as u32;
                        let g = buffer.pixels[idx + 1] as u32;
                        let b = buffer.pixels[idx + 2] as u32;

                        total_r += r;
                        total_g += g;
                        total_b += b;
                        count += 1;

                        // Threshold for "on" pixel
                        let brightness = (r + g + b) / 3;
                        if brightness > 50 {
                            let bit = match (dx, dy) {
                                (0, 0) => 0,
                                (0, 1) => 1,
                                (0, 2) => 2,
                                (1, 0) => 3,
                                (1, 1) => 4,
                                (1, 2) => 5,
                                (0, 3) => 6,
                                (1, 3) => 7,
                                _ => 0,
                            };
                            dots |= 1 << bit;
                        }
                    }
                }
            }

            // Braille Unicode block starts at U+2800
            let braille_char = char::from_u32(0x2800 + dots as u32).unwrap_or(' ');

            // Get average color
            if count > 0 {
                let r = (total_r / count) as u8;
                let g = (total_g / count) as u8;
                let b = (total_b / count) as u8;

                // ANSI 24-bit color escape sequence
                result.push_str(&format!("\x1b[38;2;{};{};{}m{}", r, g, b, braille_char));
            } else {
                result.push(' ');
            }
        }
        result.push_str("\x1b[0m\n");
    }

    (char_width, char_height, result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_braille_conversion() {
        let mut buffer = RenderBuffer::new(4, 8);
        buffer.clear(100, 100, 100);
        let (w, h, s) = buffer_to_braille(&buffer);
        assert_eq!(w, 2);
        assert_eq!(h, 2);
        assert!(!s.is_empty());
    }
}
