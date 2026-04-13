//! Font loading and management.

use std::collections::HashMap;
use std::sync::OnceLock;
use thiserror::Error;
use ttf_parser::{Face, GlyphId};

use crate::builtin::OPEN_SANS_REGULAR;

/// Errors from font operations.
#[derive(Debug, Clone, Error)]
pub enum FontError {
    /// Failed to parse font data.
    #[error("failed to parse font: {0}")]
    ParseError(String),
}

/// A parsed font that can be used for text rendering.
#[derive(Clone)]
pub struct Font {
    /// Font name.
    pub name: String,
    /// Units per em (for scaling).
    pub units_per_em: f64,
    /// Ascender height in font units.
    pub ascender: f64,
    /// Descender height in font units (typically negative).
    pub descender: f64,
    /// Line gap in font units.
    pub line_gap: f64,
    /// Raw font data (kept alive for ttf-parser).
    data: Vec<u8>,
}

impl Font {
    /// Create a font from raw TTF/OTF data.
    pub fn from_data(name: &str, data: &[u8]) -> Result<Self, FontError> {
        let face = Face::parse(data, 0).map_err(|e| FontError::ParseError(format!("{:?}", e)))?;

        let units_per_em = face.units_per_em() as f64;
        let ascender = face.ascender() as f64;
        let descender = face.descender() as f64;
        let line_gap = face.line_gap() as f64;

        Ok(Self {
            name: name.to_string(),
            units_per_em,
            ascender,
            descender,
            line_gap,
            data: data.to_vec(),
        })
    }

    /// Get the ttf-parser Face for this font.
    ///
    /// # Safety
    /// The Face borrows from self.data, so it's valid for the lifetime of self.
    pub fn face(&self) -> Face<'_> {
        // Safe because we own the data
        Face::parse(&self.data, 0).expect("already validated")
    }

    /// Get the glyph ID for a character.
    pub fn glyph_id(&self, c: char) -> Option<GlyphId> {
        self.face().glyph_index(c)
    }

    /// Get the horizontal advance width for a glyph.
    pub fn advance_width(&self, glyph_id: GlyphId) -> f64 {
        self.face()
            .glyph_hor_advance(glyph_id)
            .map(|w| w as f64)
            .unwrap_or(0.0)
    }

    /// Scale a value from font units to the given text height.
    pub fn scale_to_height(&self, value: f64, height: f64) -> f64 {
        // Height is typically the distance from descender to ascender
        let full_height = self.ascender - self.descender;
        value * height / full_height
    }
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("name", &self.name)
            .field("units_per_em", &self.units_per_em)
            .finish()
    }
}

/// Registry for managing loaded fonts.
#[derive(Debug, Default)]
pub struct FontRegistry {
    fonts: HashMap<String, Font>,
}

impl FontRegistry {
    /// Create a new empty font registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a font from raw TTF/OTF data.
    pub fn register(&mut self, name: &str, data: &[u8]) -> Result<(), FontError> {
        let font = Font::from_data(name, data)?;
        self.fonts.insert(name.to_string(), font);
        Ok(())
    }

    /// Get a font by name.
    pub fn get(&self, name: &str) -> Option<&Font> {
        self.fonts.get(name)
    }

    /// Get the built-in sans-serif font.
    pub fn builtin_sans() -> &'static Font {
        static FONT: OnceLock<Font> = OnceLock::new();
        FONT.get_or_init(|| {
            Font::from_data("sans-serif", OPEN_SANS_REGULAR).expect("built-in font should be valid")
        })
    }

    /// Get a font by name, falling back to built-in sans-serif.
    pub fn get_or_builtin(&self, name: &str) -> &Font {
        self.fonts.get(name).unwrap_or_else(|| Self::builtin_sans())
    }
}

#[cfg(all(test, not(feature = "no-builtin-font")))]
mod tests {
    use super::*;

    #[test]
    fn test_builtin_font_loads() {
        let font = FontRegistry::builtin_sans();
        assert_eq!(font.name, "sans-serif");
        assert!(font.units_per_em > 0.0);
    }

    #[test]
    fn test_glyph_lookup() {
        let font = FontRegistry::builtin_sans();
        let glyph = font.glyph_id('A');
        assert!(glyph.is_some());
    }

    #[test]
    fn test_advance_width() {
        let font = FontRegistry::builtin_sans();
        let glyph = font.glyph_id('W').unwrap();
        let width = font.advance_width(glyph);
        assert!(width > 0.0);
    }
}
