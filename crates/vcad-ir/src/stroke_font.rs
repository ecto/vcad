//! Single-stroke (Hershey-style) vector font for silkscreen text.
//!
//! This module provides a compact, dependency-free vector font where each
//! glyph is a set of polyline *strokes* — open paths traced by a pen of zero
//! width. It is intended for rendering legible PCB silkscreen legends from
//! both the Gerber writer and the PCB SVG renderer, so the two share exactly
//! the same letterforms.
//!
//! # Glyph model
//!
//! Glyphs are defined on a normalized **em box**:
//!
//! - x ranges over `0.0..=6.0` (6 em units wide)
//! - y ranges over `0.0..=7.0`, with the **baseline at `y = 0`** and the
//!   **cap height at `y = 7`** (y increases upward, CAD convention)
//!
//! A glyph is a `Vec` of strokes; each stroke is a `Vec` of `(x, y)` points
//! forming an open polyline. There are no filled regions — every character is
//! drawn with a single conceptual pen stroke (possibly lifted between
//! sub-strokes).
//!
//! Lowercase letters map to their uppercase glyphs (silkscreen text is
//! conventionally uppercase). Unknown characters render as nothing but still
//! advance the pen, so layout stays predictable.
//!
//! # Coordinates and units
//!
//! [`glyph_strokes`] and [`glyph_advance`] work in normalized em units.
//! [`text_strokes`] lays a string out left-to-right starting at the local
//! origin `(0, 0)`, scaling the em box so that the cap height equals the
//! requested `height` (in millimetres). The caller is responsible for any
//! rotation/translation into board coordinates.
//!
//! # Example
//!
//! ```
//! use vcad_ir::stroke_font::text_strokes;
//!
//! // 1.5 mm tall legend, laid out from the local origin.
//! let polylines = text_strokes("R1", 1.5);
//! assert!(!polylines.is_empty());
//! ```

/// Em box width in normalized units (each glyph cell is this wide before
/// inter-character spacing is added).
pub const EM_WIDTH: f64 = 6.0;

/// Cap height in normalized units (baseline at `y = 0`, caps reach `y = 7`).
pub const EM_HEIGHT: f64 = 7.0;

/// Horizontal gap between adjacent glyph cells, in normalized em units.
///
/// Scaled together with the glyphs by [`text_strokes`].
pub const LETTER_SPACING: f64 = 1.5;

/// A single normalized glyph: a list of polyline strokes over the em box.
type Glyph = &'static [&'static [(f64, f64)]];

/// Returns the polyline strokes for a single character in the normalized em
/// box (x in `0..=6`, y in `0..=7`, baseline at `y = 0`).
///
/// Lowercase ASCII letters are mapped to their uppercase glyph. Characters
/// with no glyph (including space and any unsupported character) return an
/// empty `Vec`.
pub fn glyph_strokes(c: char) -> Vec<Vec<(f64, f64)>> {
    glyph_data(normalize(c))
        .iter()
        .map(|stroke| stroke.to_vec())
        .collect()
}

/// Returns the horizontal advance for a single character in normalized em
/// units (the glyph cell width, **excluding** inter-character spacing).
///
/// Every supported character — including space and unknown characters —
/// advances by [`EM_WIDTH`] so that text layout is monospaced and stable.
pub fn glyph_advance(_c: char) -> f64 {
    EM_WIDTH
}

/// Lays out `text` left-to-right starting at the local origin `(0, 0)`,
/// scaling the normalized em box so the cap height equals `height` (mm).
///
/// Returns polyline strokes in local millimetre coordinates with `y`
/// increasing upward and the baseline at `y = 0`. Inter-character spacing is
/// included between glyph cells. The caller applies any rotation/translation
/// into board/world space.
///
/// A non-finite or non-positive `height` yields an empty result.
pub fn text_strokes(text: &str, height: f64) -> Vec<Vec<(f64, f64)>> {
    if !height.is_finite() || height <= 0.0 {
        return Vec::new();
    }
    let scale = height / EM_HEIGHT;
    let mut out = Vec::new();
    let mut pen_x = 0.0_f64;
    for c in text.chars() {
        for stroke in glyph_data(normalize(c)) {
            let poly: Vec<(f64, f64)> = stroke
                .iter()
                .map(|&(x, y)| ((pen_x + x) * scale, y * scale))
                .collect();
            if !poly.is_empty() {
                out.push(poly);
            }
        }
        pen_x += glyph_advance(c) + LETTER_SPACING;
    }
    out
}

/// Returns the total advance width of `text` at the given cap `height` (mm).
///
/// This is the laid-out pen width (sum of glyph advances plus inter-character
/// spacing between cells), not the inked bounding box. The trailing
/// inter-character gap after the final glyph is omitted. Returns `0.0` for an
/// empty string or a non-positive/non-finite `height`.
pub fn text_width(text: &str, height: f64) -> f64 {
    if !height.is_finite() || height <= 0.0 {
        return 0.0;
    }
    let count = text.chars().count();
    if count == 0 {
        return 0.0;
    }
    let scale = height / EM_HEIGHT;
    let advance = count as f64 * EM_WIDTH + (count as f64 - 1.0) * LETTER_SPACING;
    advance * scale
}

/// Folds a character to the key used for glyph lookup: lowercase ASCII letters
/// map to uppercase; everything else is returned unchanged.
fn normalize(c: char) -> char {
    if c.is_ascii_lowercase() {
        c.to_ascii_uppercase()
    } else {
        c
    }
}

/// Returns the static glyph data for an (already normalized) character, or an
/// empty slice if there is no glyph.
fn glyph_data(c: char) -> Glyph {
    match c {
        'A' => A,
        'B' => B,
        'C' => C,
        'D' => D,
        'E' => E,
        'F' => F,
        'G' => G,
        'H' => H,
        'I' => I,
        'J' => J,
        'K' => K,
        'L' => L,
        'M' => M,
        'N' => N,
        'O' => O,
        'P' => P,
        'Q' => Q,
        'R' => R,
        'S' => S,
        'T' => T,
        'U' => U,
        'V' => V,
        'W' => W,
        'X' => X,
        'Y' => Y,
        'Z' => Z,
        '0' => D0,
        '1' => D1,
        '2' => D2,
        '3' => D3,
        '4' => D4,
        '5' => D5,
        '6' => D6,
        '7' => D7,
        '8' => D8,
        '9' => D9,
        '-' => HYPHEN,
        '.' => PERIOD,
        ',' => COMMA,
        '/' => SLASH,
        ':' => COLON,
        '+' => PLUS,
        '(' => LPAREN,
        ')' => RPAREN,
        '°' => DEGREE,
        '%' => PERCENT,
        '#' => HASH,
        '*' => ASTERISK,
        '=' => EQUALS,
        '_' => UNDERSCORE,
        '<' => LANGLE,
        '>' => RANGLE,
        '"' => QUOTE,
        '\'' => APOS,
        _ => &[],
    }
}

// ============================================================================
// Glyph data.
//
// Each glyph is laid out on the em box: x in 0..=6, y in 0..=7, baseline y=0.
// Glyphs are kept ~1 unit inset from the cell edges (x ~0.5..5.5) so adjacent
// characters do not visually collide before LETTER_SPACING is applied.
// ============================================================================

// --- Uppercase letters ----------------------------------------------------

static A: Glyph = &[
    &[(0.5, 0.0), (3.0, 7.0), (5.5, 0.0)],
    &[(1.4, 2.5), (4.6, 2.5)],
];

static B: Glyph = &[&[
    (1.0, 0.0),
    (1.0, 7.0),
    (4.0, 7.0),
    (5.0, 6.0),
    (5.0, 4.5),
    (4.0, 3.5),
    (1.0, 3.5),
    (4.0, 3.5),
    (5.0, 2.5),
    (5.0, 1.0),
    (4.0, 0.0),
    (1.0, 0.0),
]];

static C: Glyph = &[&[
    (5.0, 5.5),
    (4.0, 7.0),
    (2.0, 7.0),
    (1.0, 5.5),
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
]];

static D: Glyph = &[&[
    (1.0, 0.0),
    (1.0, 7.0),
    (3.5, 7.0),
    (5.0, 5.5),
    (5.0, 1.5),
    (3.5, 0.0),
    (1.0, 0.0),
]];

static E: Glyph = &[
    &[(5.0, 7.0), (1.0, 7.0), (1.0, 0.0), (5.0, 0.0)],
    &[(1.0, 3.5), (4.0, 3.5)],
];

static F: Glyph = &[
    &[(5.0, 7.0), (1.0, 7.0), (1.0, 0.0)],
    &[(1.0, 3.5), (4.0, 3.5)],
];

static G: Glyph = &[&[
    (5.0, 5.5),
    (4.0, 7.0),
    (2.0, 7.0),
    (1.0, 5.5),
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
    (5.0, 3.0),
    (3.0, 3.0),
]];

static H: Glyph = &[
    &[(1.0, 0.0), (1.0, 7.0)],
    &[(5.0, 0.0), (5.0, 7.0)],
    &[(1.0, 3.5), (5.0, 3.5)],
];

static I: Glyph = &[
    &[(3.0, 0.0), (3.0, 7.0)],
    &[(1.5, 7.0), (4.5, 7.0)],
    &[(1.5, 0.0), (4.5, 0.0)],
];

static J: Glyph = &[&[(5.0, 7.0), (5.0, 1.5), (4.0, 0.0), (2.0, 0.0), (1.0, 1.5)]];

static K: Glyph = &[
    &[(1.0, 0.0), (1.0, 7.0)],
    &[(5.0, 7.0), (1.0, 3.0)],
    &[(2.5, 4.2), (5.0, 0.0)],
];

static L: Glyph = &[&[(1.0, 7.0), (1.0, 0.0), (5.0, 0.0)]];

static M: Glyph = &[&[(1.0, 0.0), (1.0, 7.0), (3.0, 3.0), (5.0, 7.0), (5.0, 0.0)]];

static N: Glyph = &[&[(1.0, 0.0), (1.0, 7.0), (5.0, 0.0), (5.0, 7.0)]];

static O: Glyph = &[&[
    (2.0, 7.0),
    (4.0, 7.0),
    (5.0, 5.5),
    (5.0, 1.5),
    (4.0, 0.0),
    (2.0, 0.0),
    (1.0, 1.5),
    (1.0, 5.5),
    (2.0, 7.0),
]];

static P: Glyph = &[&[
    (1.0, 0.0),
    (1.0, 7.0),
    (4.0, 7.0),
    (5.0, 6.0),
    (5.0, 4.5),
    (4.0, 3.5),
    (1.0, 3.5),
]];

static Q: Glyph = &[
    &[
        (2.0, 7.0),
        (4.0, 7.0),
        (5.0, 5.5),
        (5.0, 1.5),
        (4.0, 0.0),
        (2.0, 0.0),
        (1.0, 1.5),
        (1.0, 5.5),
        (2.0, 7.0),
    ],
    &[(3.5, 2.0), (5.5, 0.0)],
];

static R: Glyph = &[
    &[
        (1.0, 0.0),
        (1.0, 7.0),
        (4.0, 7.0),
        (5.0, 6.0),
        (5.0, 4.5),
        (4.0, 3.5),
        (1.0, 3.5),
    ],
    &[(3.0, 3.5), (5.0, 0.0)],
];

static S: Glyph = &[&[
    (5.0, 5.5),
    (4.0, 7.0),
    (2.0, 7.0),
    (1.0, 5.5),
    (1.0, 4.5),
    (2.0, 3.5),
    (4.0, 3.5),
    (5.0, 2.5),
    (5.0, 1.5),
    (4.0, 0.0),
    (2.0, 0.0),
    (1.0, 1.5),
]];

static T: Glyph = &[&[(0.5, 7.0), (5.5, 7.0)], &[(3.0, 7.0), (3.0, 0.0)]];

static U: Glyph = &[&[
    (1.0, 7.0),
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
    (5.0, 7.0),
]];

static V: Glyph = &[&[(1.0, 7.0), (3.0, 0.0), (5.0, 7.0)]];

static W: Glyph = &[&[(0.5, 7.0), (1.5, 0.0), (3.0, 4.0), (4.5, 0.0), (5.5, 7.0)]];

static X: Glyph = &[&[(1.0, 0.0), (5.0, 7.0)], &[(5.0, 0.0), (1.0, 7.0)]];

static Y: Glyph = &[
    &[(1.0, 7.0), (3.0, 3.5), (5.0, 7.0)],
    &[(3.0, 3.5), (3.0, 0.0)],
];

static Z: Glyph = &[&[(1.0, 7.0), (5.0, 7.0), (1.0, 0.0), (5.0, 0.0)]];

// --- Digits ----------------------------------------------------------------

static D0: Glyph = &[
    &[
        (2.0, 7.0),
        (4.0, 7.0),
        (5.0, 5.5),
        (5.0, 1.5),
        (4.0, 0.0),
        (2.0, 0.0),
        (1.0, 1.5),
        (1.0, 5.5),
        (2.0, 7.0),
    ],
    &[(1.5, 1.5), (4.5, 5.5)],
];

static D1: Glyph = &[
    &[(2.0, 5.5), (3.0, 7.0), (3.0, 0.0)],
    &[(1.5, 0.0), (4.5, 0.0)],
];

static D2: Glyph = &[&[
    (1.0, 5.5),
    (2.0, 7.0),
    (4.0, 7.0),
    (5.0, 5.5),
    (5.0, 4.5),
    (1.0, 0.0),
    (5.0, 0.0),
]];

static D3: Glyph = &[&[
    (1.0, 5.5),
    (2.0, 7.0),
    (4.0, 7.0),
    (5.0, 5.5),
    (5.0, 4.5),
    (4.0, 3.5),
    (2.5, 3.5),
    (4.0, 3.5),
    (5.0, 2.5),
    (5.0, 1.5),
    (4.0, 0.0),
    (2.0, 0.0),
    (1.0, 1.5),
]];

static D4: Glyph = &[&[(4.0, 0.0), (4.0, 7.0), (1.0, 2.5), (5.0, 2.5)]];

static D5: Glyph = &[&[
    (5.0, 7.0),
    (1.5, 7.0),
    (1.0, 3.8),
    (2.0, 4.5),
    (4.0, 4.5),
    (5.0, 3.5),
    (5.0, 1.5),
    (4.0, 0.0),
    (2.0, 0.0),
    (1.0, 1.5),
]];

static D6: Glyph = &[&[
    (5.0, 5.5),
    (4.0, 7.0),
    (2.0, 7.0),
    (1.0, 5.5),
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
    (5.0, 2.5),
    (4.0, 3.5),
    (2.0, 3.5),
    (1.0, 2.5),
]];

static D7: Glyph = &[&[(1.0, 7.0), (5.0, 7.0), (2.5, 0.0)]];

static D8: Glyph = &[&[
    (2.0, 3.5),
    (1.0, 2.5),
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
    (5.0, 2.5),
    (4.0, 3.5),
    (2.0, 3.5),
    (1.0, 4.5),
    (1.0, 5.5),
    (2.0, 7.0),
    (4.0, 7.0),
    (5.0, 5.5),
    (5.0, 4.5),
    (4.0, 3.5),
]];

static D9: Glyph = &[&[
    (1.0, 1.5),
    (2.0, 0.0),
    (4.0, 0.0),
    (5.0, 1.5),
    (5.0, 5.5),
    (4.0, 7.0),
    (2.0, 7.0),
    (1.0, 5.5),
    (1.0, 4.5),
    (2.0, 3.5),
    (4.0, 3.5),
    (5.0, 4.5),
]];

// --- Punctuation and symbols ----------------------------------------------

static HYPHEN: Glyph = &[&[(1.5, 3.5), (4.5, 3.5)]];

static PERIOD: Glyph = &[&[(2.7, 0.0), (3.3, 0.0), (3.3, 0.6), (2.7, 0.6), (2.7, 0.0)]];

static COMMA: Glyph = &[&[(3.3, 0.8), (3.3, 0.0), (2.5, -1.0)]];

static SLASH: Glyph = &[&[(1.0, 0.0), (5.0, 7.0)]];

static COLON: Glyph = &[
    &[(2.7, 1.5), (3.3, 1.5), (3.3, 2.1), (2.7, 2.1), (2.7, 1.5)],
    &[(2.7, 4.5), (3.3, 4.5), (3.3, 5.1), (2.7, 5.1), (2.7, 4.5)],
];

static PLUS: Glyph = &[&[(3.0, 1.5), (3.0, 5.5)], &[(1.0, 3.5), (5.0, 3.5)]];

static LPAREN: Glyph = &[&[(4.0, 7.0), (2.0, 5.0), (2.0, 2.0), (4.0, 0.0)]];

static RPAREN: Glyph = &[&[(2.0, 7.0), (4.0, 5.0), (4.0, 2.0), (2.0, 0.0)]];

static DEGREE: Glyph = &[&[
    (3.0, 7.0),
    (3.8, 6.6),
    (3.8, 5.8),
    (3.0, 5.4),
    (2.2, 5.8),
    (2.2, 6.6),
    (3.0, 7.0),
]];

static PERCENT: Glyph = &[
    &[(1.0, 0.0), (5.0, 7.0)],
    &[(1.5, 7.0), (2.3, 7.0), (2.3, 5.8), (1.5, 5.8), (1.5, 7.0)],
    &[(3.7, 1.2), (4.5, 1.2), (4.5, 0.0), (3.7, 0.0), (3.7, 1.2)],
];

static HASH: Glyph = &[
    &[(2.0, 0.0), (2.8, 7.0)],
    &[(3.8, 0.0), (4.6, 7.0)],
    &[(1.0, 2.3), (5.5, 2.3)],
    &[(1.0, 4.7), (5.5, 4.7)],
];

static ASTERISK: Glyph = &[
    &[(3.0, 2.0), (3.0, 7.0)],
    &[(1.0, 3.0), (5.0, 6.0)],
    &[(5.0, 3.0), (1.0, 6.0)],
];

static EQUALS: Glyph = &[&[(1.0, 2.7), (5.0, 2.7)], &[(1.0, 4.3), (5.0, 4.3)]];

static UNDERSCORE: Glyph = &[&[(0.5, 0.0), (5.5, 0.0)]];

static LANGLE: Glyph = &[&[(4.5, 6.0), (1.5, 3.5), (4.5, 1.0)]];

static RANGLE: Glyph = &[&[(1.5, 6.0), (4.5, 3.5), (1.5, 1.0)]];

static QUOTE: Glyph = &[&[(2.3, 7.0), (2.3, 5.5)], &[(3.7, 7.0), (3.7, 5.5)]];

static APOS: Glyph = &[&[(3.0, 7.0), (3.0, 5.5)]];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required glyph must have at least one stroke.
    #[test]
    fn required_glyphs_have_strokes() {
        let required: Vec<char> = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.,/:+()°%"
            .chars()
            .collect();
        for c in required {
            let strokes = glyph_strokes(c);
            assert!(
                !strokes.is_empty(),
                "glyph {:?} should have at least one stroke",
                c
            );
            for stroke in &strokes {
                assert!(
                    stroke.len() >= 2,
                    "glyph {:?} has a stroke with fewer than 2 points",
                    c
                );
            }
        }
    }

    /// Space and unknown characters render as nothing but still advance.
    #[test]
    fn space_and_unknown_render_nothing() {
        assert!(glyph_strokes(' ').is_empty());
        assert!(glyph_strokes('\u{1F600}').is_empty());
        // But they still advance, so layout stays predictable.
        assert_eq!(glyph_advance(' '), EM_WIDTH);
        assert_eq!(glyph_advance('\u{1F600}'), EM_WIDTH);
    }

    /// 'A' at cap height 1.0 stays within sane bounds: x roughly within the em
    /// cell and y within `0..=1`.
    #[test]
    fn glyph_a_bounds_at_height_one() {
        let strokes = text_strokes("A", 1.0);
        assert!(!strokes.is_empty());
        for stroke in &strokes {
            for &(x, y) in stroke {
                // Cap height is 1.0, so y must be within [0, 1].
                assert!((0.0..=1.0).contains(&y), "y {} out of [0,1]", y);
                // x must stay within the scaled em cell width.
                let max_x = EM_WIDTH / EM_HEIGHT; // em cell at height 1.0
                assert!(
                    (0.0..=max_x + 1e-9).contains(&x),
                    "x {} out of [0,{}]",
                    x,
                    max_x
                );
            }
        }
    }

    /// "AB" must advance: the second glyph lies entirely to the right of the
    /// first glyph's rightmost inked point.
    #[test]
    fn text_advances_left_to_right() {
        let single = text_strokes("A", 2.0);
        let pair = text_strokes("AB", 2.0);
        assert!(pair.len() > single.len(), "pair should add B's strokes");

        let a_max_x = single
            .iter()
            .flat_map(|s| s.iter())
            .map(|&(x, _)| x)
            .fold(f64::MIN, f64::max);

        // Strokes in `pair` beyond those of "A" belong to "B"; the smallest x
        // among the B strokes should be to the right of A's max x (accounting
        // for the cell advance + spacing).
        let scale = 2.0 / EM_HEIGHT;
        let cell_advance = (EM_WIDTH + LETTER_SPACING) * scale;
        let b_min_x = pair[single.len()..]
            .iter()
            .flat_map(|s| s.iter())
            .map(|&(x, _)| x)
            .fold(f64::MAX, f64::min);

        assert!(
            b_min_x > a_max_x,
            "B (min x {}) should be right of A (max x {})",
            b_min_x,
            a_max_x
        );
        assert!(
            b_min_x >= cell_advance - 1e-9,
            "B should start at least one cell advance ({}) in, got {}",
            cell_advance,
            b_min_x
        );
    }

    /// Lowercase letters map to their uppercase glyphs.
    #[test]
    fn lowercase_maps_to_uppercase() {
        for (lo, up) in [('a', 'A'), ('m', 'M'), ('z', 'Z'), ('r', 'R')] {
            assert_eq!(
                glyph_strokes(lo),
                glyph_strokes(up),
                "lowercase {:?} should match uppercase {:?}",
                lo,
                up
            );
        }
    }

    /// `text_strokes` scales with `height`: doubling height doubles all coords.
    #[test]
    fn height_scales_linearly() {
        let small = text_strokes("E", 1.0);
        let big = text_strokes("E", 2.0);
        assert_eq!(small.len(), big.len());
        for (s, b) in small.iter().zip(big.iter()) {
            for (&(sx, sy), &(bx, by)) in s.iter().zip(b.iter()) {
                assert!((bx - 2.0 * sx).abs() < 1e-9);
                assert!((by - 2.0 * sy).abs() < 1e-9);
            }
        }
    }

    /// `text_width` matches the laid-out advance and rejects bad heights.
    #[test]
    fn text_width_is_consistent() {
        assert_eq!(text_width("", 2.0), 0.0);
        assert_eq!(text_width("A", -1.0), 0.0);
        assert_eq!(text_width("A", f64::NAN), 0.0);

        // Single glyph: one cell, no trailing spacing.
        let scale = 2.0 / EM_HEIGHT;
        assert!((text_width("A", 2.0) - EM_WIDTH * scale).abs() < 1e-9);

        // Two glyphs: two cells plus one spacing gap.
        let expected = (2.0 * EM_WIDTH + LETTER_SPACING) * scale;
        assert!((text_width("AB", 2.0) - expected).abs() < 1e-9);
    }

    /// Non-positive or non-finite heights produce no strokes.
    #[test]
    fn bad_height_yields_no_strokes() {
        assert!(text_strokes("ABC", 0.0).is_empty());
        assert!(text_strokes("ABC", -2.0).is_empty());
        assert!(text_strokes("ABC", f64::INFINITY).is_empty());
    }
}
