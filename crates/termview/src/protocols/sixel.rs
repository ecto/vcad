//! Sixel graphics protocol implementation.
//!
//! Sixel is a bitmap graphics format for terminals, originally from DEC VT series.
//! It's widely supported by terminal emulators like xterm, mlterm, foot, and others.
//!
//! Format: Each Sixel character represents 6 vertical pixels (hence "sixel").

use std::collections::HashMap;

/// Sixel image encoder.
pub struct SixelEncoder {
    width: u32,
    height: u32,
    palette: Vec<[u8; 3]>,
}

impl SixelEncoder {
    /// Create a new Sixel encoder for the given dimensions.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            palette: Vec::new(),
        }
    }

    /// Encode RGBA buffer to Sixel format.
    ///
    /// Returns a string containing the complete Sixel escape sequence.
    pub fn encode(&mut self, pixels: &[u8]) -> String {
        // Sixel format:
        // ESC P q <data> ESC \
        // where q = aspect ratio (0;0;0 = 1:1)
        // Data is rows of 6 pixels encoded as characters 63-126

        // Build color palette (quantize to 64 colors)
        let indexed = self.quantize_colors(pixels);

        let mut output = String::new();

        // Start Sixel sequence with 1:1 aspect ratio
        output.push_str("\x1bPq");

        // Define palette
        for (i, color) in self.palette.iter().enumerate() {
            // #N;2;R;G;B (RGB as 0-100 percentages)
            output.push_str(&format!(
                "#{};2;{};{};{}",
                i,
                (color[0] as u32 * 100) / 255,
                (color[1] as u32 * 100) / 255,
                (color[2] as u32 * 100) / 255,
            ));
        }

        // Encode pixel data in 6-row bands
        for band_y in (0..self.height).step_by(6) {
            for color_idx in 0..self.palette.len() {
                let mut run_length = 0u32;
                let mut last_sixel = 0u8;
                let mut has_content = false;

                // Start this color
                output.push_str(&format!("#{}", color_idx));

                for x in 0..self.width {
                    let mut sixel = 0u8;
                    for dy in 0..6 {
                        let y = band_y + dy;
                        if y < self.height {
                            let idx = (y * self.width + x) as usize;
                            if idx < indexed.len() && indexed[idx] == color_idx as u8 {
                                sixel |= 1 << dy;
                                has_content = true;
                            }
                        }
                    }

                    if sixel == last_sixel && run_length < 255 {
                        run_length += 1;
                    } else {
                        if run_length > 0 {
                            self.emit_sixel(&mut output, last_sixel, run_length);
                        }
                        last_sixel = sixel;
                        run_length = 1;
                    }
                }

                if run_length > 0 {
                    self.emit_sixel(&mut output, last_sixel, run_length);
                }

                // Only output carriage return if we had content
                if has_content {
                    output.push('$'); // Carriage return (same line)
                }
            }
            output.push('-'); // Line feed (next band)
        }

        // End Sixel sequence
        output.push_str("\x1b\\");

        output
    }

    fn emit_sixel(&self, output: &mut String, sixel: u8, count: u32) {
        let ch = (sixel + 63) as char;
        if count > 3 {
            // Use run-length encoding
            output.push_str(&format!("!{}{}", count, ch));
        } else {
            for _ in 0..count {
                output.push(ch);
            }
        }
    }

    fn quantize_colors(&mut self, pixels: &[u8]) -> Vec<u8> {
        // Simple median-cut quantization to 64 colors
        // Returns indexed pixel array

        // Count unique colors
        let mut color_counts: HashMap<[u8; 3], u32> = HashMap::new();
        for chunk in pixels.chunks(4) {
            if chunk.len() >= 4 && chunk[3] > 128 {
                // Skip transparent
                let color = [chunk[0], chunk[1], chunk[2]];
                *color_counts.entry(color).or_insert(0) += 1;
            }
        }

        // Take top 64 colors by frequency
        let mut colors: Vec<_> = color_counts.into_iter().collect();
        colors.sort_by_key(|c| std::cmp::Reverse(c.1));
        colors.truncate(64);

        self.palette = colors.iter().map(|(c, _)| *c).collect();
        if self.palette.is_empty() {
            self.palette.push([0, 0, 0]); // Ensure at least one color
        }

        // Map pixels to palette indices
        let mut indexed = Vec::with_capacity((self.width * self.height) as usize);
        for chunk in pixels.chunks(4) {
            if chunk.len() >= 3 {
                let color = [chunk[0], chunk[1], chunk[2]];
                let idx = self.find_nearest_color(&color);
                indexed.push(idx);
            } else {
                indexed.push(0);
            }
        }

        indexed
    }

    fn find_nearest_color(&self, color: &[u8; 3]) -> u8 {
        let mut best_idx = 0;
        let mut best_dist = u32::MAX;

        for (i, palette_color) in self.palette.iter().enumerate() {
            let dr = (color[0] as i32 - palette_color[0] as i32).unsigned_abs();
            let dg = (color[1] as i32 - palette_color[1] as i32).unsigned_abs();
            let db = (color[2] as i32 - palette_color[2] as i32).unsigned_abs();
            let dist = dr * dr + dg * dg + db * db;

            if dist < best_dist {
                best_dist = dist;
                best_idx = i;
            }
        }

        best_idx as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sixel_encoder_creation() {
        let encoder = SixelEncoder::new(100, 100);
        assert_eq!(encoder.width, 100);
        assert_eq!(encoder.height, 100);
    }

    #[test]
    fn test_sixel_encode_simple() {
        let mut encoder = SixelEncoder::new(2, 6);
        // Create a 2x6 red image (RGBA)
        let pixels = vec![255, 0, 0, 255].repeat(12);
        let result = encoder.encode(&pixels);

        // Should start with ESC P q
        assert!(result.starts_with("\x1bPq"));
        // Should end with ESC \
        assert!(result.ends_with("\x1b\\"));
    }

    #[test]
    fn test_color_quantization() {
        let mut encoder = SixelEncoder::new(2, 2);
        let pixels = vec![
            255, 0, 0, 255, // Red
            0, 255, 0, 255, // Green
            0, 0, 255, 255, // Blue
            255, 255, 0, 255, // Yellow
        ];
        let indexed = encoder.quantize_colors(&pixels);
        assert_eq!(indexed.len(), 4);
        assert_eq!(encoder.palette.len(), 4);
    }
}
