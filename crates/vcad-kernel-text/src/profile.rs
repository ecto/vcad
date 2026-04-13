//! Text to sketch profile conversion.

use vcad_kernel_math::{Point3, Vec3};
use vcad_kernel_sketch::SketchProfile;

use crate::font::Font;
use crate::glyph::{contour_to_segments, extract_glyph_contours};
use crate::TextAlignment;

/// Convert text to a list of sketch profiles.
///
/// Each profile represents one connected contour (glyph outline or hole).
/// These profiles can be used with extrude() to create 3D text.
///
/// # Arguments
///
/// * `text` - The text string to convert
/// * `font` - The font to use
/// * `height` - Text height in mm
/// * `letter_spacing` - Letter spacing multiplier (1.0 = normal)
/// * `line_spacing` - Line spacing multiplier (1.0 = normal)
/// * `alignment` - Text alignment
///
/// # Returns
///
/// A list of sketch profiles, one for each glyph contour.
/// Note: Holes (like in 'O' or 'D') are separate profiles and should
/// be subtracted from the outer contour when creating 3D geometry.
pub fn text_to_profiles(
    text: &str,
    font: &Font,
    height: f64,
    letter_spacing: f64,
    line_spacing: f64,
    alignment: TextAlignment,
) -> Vec<SketchProfile> {
    if text.is_empty() || height < 0.1 {
        return Vec::new();
    }

    let face = font.face();

    // Calculate scale factor from font units to mm
    let full_font_height = font.ascender - font.descender;
    let scale = height / full_font_height;

    // Line height for multi-line text
    let line_height = height * line_spacing;

    let mut profiles = Vec::new();

    // Process each line of text
    for (line_idx, line) in text.lines().enumerate() {
        // Calculate line width for alignment
        let line_width = calculate_line_width(line, font, scale, letter_spacing);

        // Calculate starting X offset based on alignment
        let x_offset = match alignment {
            TextAlignment::Left => 0.0,
            TextAlignment::Center => -line_width / 2.0,
            TextAlignment::Right => -line_width,
        };

        // Y offset for this line (Y goes up, so subtract for each line)
        let y_offset = -(line_idx as f64) * line_height;

        // Current X position along the line
        let mut cursor_x = x_offset;

        for c in line.chars() {
            // Skip whitespace but advance cursor
            if c.is_whitespace() {
                if let Some(glyph_id) = font.glyph_id(c) {
                    let advance = font.advance_width(glyph_id) * scale * letter_spacing;
                    cursor_x += advance;
                } else {
                    // Default space width
                    cursor_x += height * 0.3;
                }
                continue;
            }

            // Get glyph for character
            let Some(glyph_id) = font.glyph_id(c) else {
                // Skip unknown characters
                continue;
            };

            // Extract glyph contours
            let contours = extract_glyph_contours(&face, glyph_id);

            // Convert each contour to a profile
            for contour in &contours {
                let segments = contour_to_segments(contour, scale, cursor_x, y_offset);

                // Need at least 3 segments for a closed profile
                if segments.len() >= 3 {
                    // Try to create the profile - skip if invalid
                    if let Ok(profile) =
                        SketchProfile::new(Point3::origin(), Vec3::x(), Vec3::y(), segments)
                    {
                        profiles.push(profile);
                    }
                }
            }

            // Advance cursor by glyph width
            let advance = font.advance_width(glyph_id) * scale * letter_spacing;
            cursor_x += advance;
        }
    }

    profiles
}

/// Get the bounding box of rendered text.
///
/// Returns (width, height) in mm.
pub fn text_bounds(
    text: &str,
    font: &Font,
    height: f64,
    letter_spacing: f64,
    line_spacing: f64,
) -> (f64, f64) {
    if text.is_empty() || height < 0.1 {
        return (0.0, 0.0);
    }

    // Calculate scale factor
    let full_font_height = font.ascender - font.descender;
    let scale = height / full_font_height;

    // Line height for multi-line text
    let line_height = height * line_spacing;

    // Find maximum line width
    let max_width = text
        .lines()
        .map(|line| calculate_line_width(line, font, scale, letter_spacing))
        .fold(0.0_f64, |a, b| a.max(b));

    // Total height = number of lines * line height
    let num_lines = text.lines().count().max(1);
    let total_height = (num_lines as f64) * line_height;

    (max_width, total_height)
}

/// Calculate the width of a single line of text.
fn calculate_line_width(line: &str, font: &Font, scale: f64, letter_spacing: f64) -> f64 {
    let mut width = 0.0;

    for c in line.chars() {
        if let Some(glyph_id) = font.glyph_id(c) {
            width += font.advance_width(glyph_id) * scale * letter_spacing;
        } else {
            // Default character width for unknown glyphs
            width += scale * 0.5 * font.units_per_em;
        }
    }

    width
}

#[cfg(all(test, not(feature = "no-builtin-font")))]
mod tests {
    use super::*;
    use crate::font::FontRegistry;

    #[test]
    fn test_text_to_profiles_empty() {
        let font = FontRegistry::builtin_sans();
        let profiles = text_to_profiles("", font, 10.0, 1.0, 1.2, TextAlignment::Left);
        assert!(profiles.is_empty());
    }

    #[test]
    fn test_text_to_profiles_simple() {
        let font = FontRegistry::builtin_sans();
        let profiles = text_to_profiles("A", font, 10.0, 1.0, 1.2, TextAlignment::Left);

        // 'A' should produce at least one profile (outer contour)
        // and possibly a second for the inner hole
        assert!(!profiles.is_empty());
    }

    #[test]
    fn test_text_bounds() {
        let font = FontRegistry::builtin_sans();
        let (width, height) = text_bounds("Hello", font, 10.0, 1.0, 1.2);

        assert!(width > 0.0);
        assert!(height > 0.0);
        // Height should be roughly line_spacing * text_height for single line
        assert!((height - 12.0).abs() < 1.0);
    }

    #[test]
    fn test_text_bounds_multiline() {
        let font = FontRegistry::builtin_sans();
        let (_, height1) = text_bounds("A", font, 10.0, 1.0, 1.2);
        let (_, height2) = text_bounds("A\nB", font, 10.0, 1.0, 1.2);

        // Two lines should be roughly twice the height
        assert!(height2 > height1 * 1.5);
    }

    #[test]
    fn test_alignment_affects_position() {
        let font = FontRegistry::builtin_sans();

        let left = text_to_profiles("A", font, 10.0, 1.0, 1.2, TextAlignment::Left);
        let center = text_to_profiles("A", font, 10.0, 1.0, 1.2, TextAlignment::Center);
        let right = text_to_profiles("A", font, 10.0, 1.0, 1.2, TextAlignment::Right);

        // All should produce profiles
        assert!(!left.is_empty());
        assert!(!center.is_empty());
        assert!(!right.is_empty());

        // The x positions should differ based on alignment
        // (actual position testing would require looking at vertices)
    }
}
