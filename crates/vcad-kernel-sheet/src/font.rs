//! Single-stroke (Hershey-style) vector font for laser engraving.
//!
//! Fab services' engraving/marking pass consumes vector polylines — a
//! filled TrueType outline would double-burn every stroke edge. This
//! module renders text as open polylines: each glyph is a small set of
//! pen strokes on an 8-unit-tall design grid, scaled so the cap height
//! equals the requested text height.
//!
//! Coverage is deliberately small — `A–Z`, `0–9`, and the punctuation
//! that shows up in part labels (`- . / # + :` and space). Lowercase
//! input is upcased. Unsupported characters are a hard error (fail
//! closed): a part label that silently dropped a character would be
//! worse than one that failed to build.

use vcad_kernel_math::Point2;

/// Design-grid cap height. All glyph coordinates live in `y ∈ [0, 8]`.
const CAP: f64 = 8.0;
/// Horizontal gap between glyphs, in design-grid units.
const SPACING: f64 = 2.0;

/// Errors from [`text_to_polylines`].
#[derive(Debug, Clone, PartialEq)]
pub enum FontError {
    /// A character with no glyph in the engraving font.
    UnsupportedChar(char),
    /// Text height must be > 0.
    InvalidHeight(f64),
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::UnsupportedChar(c) => write!(
                f,
                "no engraving glyph for {c:?} (supported: A-Z, 0-9, space, and \"-./#+:\")"
            ),
            FontError::InvalidHeight(h) => write!(f, "text height must be > 0, got {h}"),
        }
    }
}

impl std::error::Error for FontError {}

/// One glyph: advance width on the design grid + pen strokes.
struct Glyph {
    width: f64,
    strokes: &'static [&'static [(f64, f64)]],
}

/// Look up the glyph for `c` (already upcased).
fn glyph(c: char) -> Option<Glyph> {
    let g = |width: f64, strokes: &'static [&'static [(f64, f64)]]| Some(Glyph { width, strokes });
    match c {
        'A' => g(
            6.0,
            &[
                &[(0.0, 0.0), (3.0, 8.0), (6.0, 0.0)],
                &[(1.5, 4.0), (4.5, 4.0)],
            ],
        ),
        'B' => g(
            6.0,
            &[
                &[
                    (0.0, 0.0),
                    (0.0, 8.0),
                    (5.0, 8.0),
                    (6.0, 7.0),
                    (6.0, 5.0),
                    (5.0, 4.0),
                    (0.0, 4.0),
                ],
                &[(5.0, 4.0), (6.0, 3.0), (6.0, 1.0), (5.0, 0.0), (0.0, 0.0)],
            ],
        ),
        'C' => g(
            6.0,
            &[&[
                (6.0, 7.0),
                (5.0, 8.0),
                (1.0, 8.0),
                (0.0, 7.0),
                (0.0, 1.0),
                (1.0, 0.0),
                (5.0, 0.0),
                (6.0, 1.0),
            ]],
        ),
        'D' => g(
            6.0,
            &[&[
                (0.0, 0.0),
                (0.0, 8.0),
                (4.0, 8.0),
                (6.0, 6.0),
                (6.0, 2.0),
                (4.0, 0.0),
                (0.0, 0.0),
            ]],
        ),
        'E' => g(
            6.0,
            &[
                &[(6.0, 8.0), (0.0, 8.0), (0.0, 0.0), (6.0, 0.0)],
                &[(0.0, 4.0), (4.0, 4.0)],
            ],
        ),
        'F' => g(
            6.0,
            &[
                &[(6.0, 8.0), (0.0, 8.0), (0.0, 0.0)],
                &[(0.0, 4.0), (4.0, 4.0)],
            ],
        ),
        'G' => g(
            6.0,
            &[&[
                (6.0, 7.0),
                (5.0, 8.0),
                (1.0, 8.0),
                (0.0, 7.0),
                (0.0, 1.0),
                (1.0, 0.0),
                (5.0, 0.0),
                (6.0, 1.0),
                (6.0, 4.0),
                (3.0, 4.0),
            ]],
        ),
        'H' => g(
            6.0,
            &[
                &[(0.0, 0.0), (0.0, 8.0)],
                &[(6.0, 0.0), (6.0, 8.0)],
                &[(0.0, 4.0), (6.0, 4.0)],
            ],
        ),
        'I' => g(
            2.0,
            &[
                &[(0.0, 8.0), (2.0, 8.0)],
                &[(1.0, 8.0), (1.0, 0.0)],
                &[(0.0, 0.0), (2.0, 0.0)],
            ],
        ),
        'J' => g(
            5.0,
            &[&[(5.0, 8.0), (5.0, 1.0), (4.0, 0.0), (1.0, 0.0), (0.0, 1.0)]],
        ),
        'K' => g(
            6.0,
            &[
                &[(0.0, 0.0), (0.0, 8.0)],
                &[(6.0, 8.0), (0.0, 3.0)],
                &[(6.0, 0.0), (0.0, 3.0)],
            ],
        ),
        'L' => g(6.0, &[&[(0.0, 8.0), (0.0, 0.0), (6.0, 0.0)]]),
        'M' => g(
            6.0,
            &[&[(0.0, 0.0), (0.0, 8.0), (3.0, 3.0), (6.0, 8.0), (6.0, 0.0)]],
        ),
        'N' => g(6.0, &[&[(0.0, 0.0), (0.0, 8.0), (6.0, 0.0), (6.0, 8.0)]]),
        'O' => g(
            6.0,
            &[&[
                (1.0, 0.0),
                (0.0, 1.0),
                (0.0, 7.0),
                (1.0, 8.0),
                (5.0, 8.0),
                (6.0, 7.0),
                (6.0, 1.0),
                (5.0, 0.0),
                (1.0, 0.0),
            ]],
        ),
        'P' => g(
            6.0,
            &[&[
                (0.0, 0.0),
                (0.0, 8.0),
                (5.0, 8.0),
                (6.0, 7.0),
                (6.0, 5.0),
                (5.0, 4.0),
                (0.0, 4.0),
            ]],
        ),
        'Q' => g(
            6.0,
            &[
                &[
                    (1.0, 0.0),
                    (0.0, 1.0),
                    (0.0, 7.0),
                    (1.0, 8.0),
                    (5.0, 8.0),
                    (6.0, 7.0),
                    (6.0, 1.0),
                    (5.0, 0.0),
                    (1.0, 0.0),
                ],
                &[(4.0, 2.0), (6.5, -0.5)],
            ],
        ),
        'R' => g(
            6.0,
            &[
                &[
                    (0.0, 0.0),
                    (0.0, 8.0),
                    (5.0, 8.0),
                    (6.0, 7.0),
                    (6.0, 5.0),
                    (5.0, 4.0),
                    (0.0, 4.0),
                ],
                &[(3.0, 4.0), (6.0, 0.0)],
            ],
        ),
        'S' => g(
            6.0,
            &[&[
                (6.0, 7.0),
                (5.0, 8.0),
                (1.0, 8.0),
                (0.0, 7.0),
                (0.0, 5.0),
                (1.0, 4.0),
                (5.0, 4.0),
                (6.0, 3.0),
                (6.0, 1.0),
                (5.0, 0.0),
                (1.0, 0.0),
                (0.0, 1.0),
            ]],
        ),
        'T' => g(6.0, &[&[(0.0, 8.0), (6.0, 8.0)], &[(3.0, 8.0), (3.0, 0.0)]]),
        'U' => g(
            6.0,
            &[&[
                (0.0, 8.0),
                (0.0, 1.0),
                (1.0, 0.0),
                (5.0, 0.0),
                (6.0, 1.0),
                (6.0, 8.0),
            ]],
        ),
        'V' => g(6.0, &[&[(0.0, 8.0), (3.0, 0.0), (6.0, 8.0)]]),
        'W' => g(
            6.0,
            &[&[(0.0, 8.0), (1.5, 0.0), (3.0, 6.0), (4.5, 0.0), (6.0, 8.0)]],
        ),
        'X' => g(6.0, &[&[(0.0, 0.0), (6.0, 8.0)], &[(0.0, 8.0), (6.0, 0.0)]]),
        'Y' => g(
            6.0,
            &[
                &[(0.0, 8.0), (3.0, 4.0), (6.0, 8.0)],
                &[(3.0, 4.0), (3.0, 0.0)],
            ],
        ),
        'Z' => g(6.0, &[&[(0.0, 8.0), (6.0, 8.0), (0.0, 0.0), (6.0, 0.0)]]),
        '0' => g(
            6.0,
            &[
                &[
                    (1.0, 0.0),
                    (0.0, 1.0),
                    (0.0, 7.0),
                    (1.0, 8.0),
                    (5.0, 8.0),
                    (6.0, 7.0),
                    (6.0, 1.0),
                    (5.0, 0.0),
                    (1.0, 0.0),
                ],
                &[(1.0, 1.0), (5.0, 7.0)],
            ],
        ),
        '1' => g(
            4.0,
            &[
                &[(0.0, 6.0), (2.0, 8.0), (2.0, 0.0)],
                &[(0.0, 0.0), (4.0, 0.0)],
            ],
        ),
        '2' => g(
            6.0,
            &[&[
                (0.0, 7.0),
                (1.0, 8.0),
                (5.0, 8.0),
                (6.0, 7.0),
                (6.0, 5.0),
                (0.0, 0.0),
                (6.0, 0.0),
            ]],
        ),
        '3' => g(
            6.0,
            &[
                &[
                    (0.0, 7.0),
                    (1.0, 8.0),
                    (5.0, 8.0),
                    (6.0, 7.0),
                    (6.0, 5.0),
                    (5.0, 4.0),
                    (2.0, 4.0),
                ],
                &[
                    (5.0, 4.0),
                    (6.0, 3.0),
                    (6.0, 1.0),
                    (5.0, 0.0),
                    (1.0, 0.0),
                    (0.0, 1.0),
                ],
            ],
        ),
        '4' => g(6.0, &[&[(4.0, 0.0), (4.0, 8.0), (0.0, 2.0), (6.0, 2.0)]]),
        '5' => g(
            6.0,
            &[&[
                (6.0, 8.0),
                (0.0, 8.0),
                (0.0, 4.0),
                (4.0, 4.0),
                (6.0, 3.0),
                (6.0, 1.0),
                (5.0, 0.0),
                (1.0, 0.0),
                (0.0, 1.0),
            ]],
        ),
        '6' => g(
            6.0,
            &[&[
                (5.0, 8.0),
                (1.0, 8.0),
                (0.0, 7.0),
                (0.0, 1.0),
                (1.0, 0.0),
                (5.0, 0.0),
                (6.0, 1.0),
                (6.0, 3.0),
                (5.0, 4.0),
                (0.0, 4.0),
            ]],
        ),
        '7' => g(6.0, &[&[(0.0, 8.0), (6.0, 8.0), (2.0, 0.0)]]),
        '8' => g(
            6.0,
            &[&[
                (1.0, 4.0),
                (0.0, 5.0),
                (0.0, 7.0),
                (1.0, 8.0),
                (5.0, 8.0),
                (6.0, 7.0),
                (6.0, 5.0),
                (5.0, 4.0),
                (1.0, 4.0),
                (0.0, 3.0),
                (0.0, 1.0),
                (1.0, 0.0),
                (5.0, 0.0),
                (6.0, 1.0),
                (6.0, 3.0),
                (5.0, 4.0),
            ]],
        ),
        '9' => g(
            6.0,
            &[&[
                (6.0, 4.0),
                (1.0, 4.0),
                (0.0, 5.0),
                (0.0, 7.0),
                (1.0, 8.0),
                (5.0, 8.0),
                (6.0, 7.0),
                (6.0, 1.0),
                (5.0, 0.0),
                (1.0, 0.0),
            ]],
        ),
        '-' => g(4.0, &[&[(0.0, 4.0), (4.0, 4.0)]]),
        '.' => g(
            1.0,
            &[&[(0.0, 0.0), (1.0, 0.0), (1.0, 1.0), (0.0, 1.0), (0.0, 0.0)]],
        ),
        '/' => g(6.0, &[&[(0.0, 0.0), (6.0, 8.0)]]),
        '#' => g(
            6.0,
            &[
                &[(2.0, 0.0), (2.0, 8.0)],
                &[(4.0, 0.0), (4.0, 8.0)],
                &[(0.0, 3.0), (6.0, 3.0)],
                &[(0.0, 5.0), (6.0, 5.0)],
            ],
        ),
        '+' => g(6.0, &[&[(3.0, 1.0), (3.0, 7.0)], &[(0.0, 4.0), (6.0, 4.0)]]),
        ':' => g(
            1.0,
            &[
                &[(0.0, 1.0), (1.0, 1.0), (1.0, 2.0), (0.0, 2.0), (0.0, 1.0)],
                &[(0.0, 5.0), (1.0, 5.0), (1.0, 6.0), (0.0, 6.0), (0.0, 5.0)],
            ],
        ),
        ' ' => g(4.0, &[]),
        _ => None,
    }
}

/// Render `text` as open polylines in mm.
///
/// The text baseline starts at `(x, y)` and runs along `angle_rad`
/// (radians, counter-clockwise from +X; `0.0` = left-to-right). `height`
/// is the cap height in mm. Lowercase letters are upcased; unsupported
/// characters are a [`FontError::UnsupportedChar`] error.
pub fn text_to_polylines(
    text: &str,
    x: f64,
    y: f64,
    height: f64,
    angle_rad: f64,
) -> Result<Vec<Vec<Point2>>, FontError> {
    if height <= 0.0 || height.is_nan() {
        return Err(FontError::InvalidHeight(height));
    }
    let scale = height / CAP;
    let (sin, cos) = angle_rad.sin_cos();
    let mut polylines = Vec::new();
    let mut pen_x = 0.0; // along the baseline, in design-grid units
    for c in text.chars() {
        let up = c.to_ascii_uppercase();
        let glyph = glyph(up).ok_or(FontError::UnsupportedChar(c))?;
        for stroke in glyph.strokes {
            let pts: Vec<Point2> = stroke
                .iter()
                .map(|&(gx, gy)| {
                    // Design grid → mm, then rotate about the anchor and translate.
                    let lx = (pen_x + gx) * scale;
                    let ly = gy * scale;
                    Point2::new(x + lx * cos - ly * sin, y + lx * sin + ly * cos)
                })
                .collect();
            if pts.len() >= 2 {
                polylines.push(pts);
            }
        }
        pen_x += glyph.width + SPACING;
    }
    Ok(polylines)
}

/// Total advance width (mm) of `text` at the given cap height — the same
/// metric [`text_to_polylines`] uses, minus the trailing inter-glyph gap.
/// Handy for centering a label on a flange.
pub fn text_width(text: &str, height: f64) -> Result<f64, FontError> {
    if height <= 0.0 || height.is_nan() {
        return Err(FontError::InvalidHeight(height));
    }
    let scale = height / CAP;
    let mut w = 0.0;
    let mut count = 0usize;
    for c in text.chars() {
        let glyph = glyph(c.to_ascii_uppercase()).ok_or(FontError::UnsupportedChar(c))?;
        w += glyph.width + SPACING;
        count += 1;
    }
    if count > 0 {
        w -= SPACING;
    }
    Ok(w * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn part_labels_render() {
        for label in ["A4", "G4", "C5", "REV B", "P/N 12-034", "#3"] {
            let polylines = text_to_polylines(label, 0.0, 0.0, 6.0, 0.0).unwrap();
            assert!(!polylines.is_empty(), "{label} produced no strokes");
            for pl in &polylines {
                assert!(pl.len() >= 2);
            }
        }
    }

    #[test]
    fn every_glyph_stays_on_the_design_grid() {
        // All strokes within [−1, width+1] × [−1, 9] (Q's tail dips below
        // the baseline slightly; nothing else strays).
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-./#+: ".chars() {
            let g = glyph(c).unwrap_or_else(|| panic!("missing glyph {c:?}"));
            for stroke in g.strokes {
                for &(x, y) in *stroke {
                    assert!((-1.0..=g.width + 1.0).contains(&x), "{c:?} x={x}");
                    assert!((-1.0..=9.0).contains(&y), "{c:?} y={y}");
                }
            }
        }
    }

    #[test]
    fn height_sets_cap_height() {
        let polylines = text_to_polylines("T", 0.0, 0.0, 10.0, 0.0).unwrap();
        let max_y = polylines
            .iter()
            .flatten()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!((max_y - 10.0).abs() < 1e-9);
    }

    #[test]
    fn lowercase_is_upcased() {
        let lower = text_to_polylines("a4", 0.0, 0.0, 6.0, 0.0).unwrap();
        let upper = text_to_polylines("A4", 0.0, 0.0, 6.0, 0.0).unwrap();
        assert_eq!(lower, upper);
    }

    #[test]
    fn unsupported_char_is_a_hard_error() {
        assert_eq!(
            text_to_polylines("Ω", 0.0, 0.0, 6.0, 0.0),
            Err(FontError::UnsupportedChar('Ω'))
        );
        assert!(matches!(
            text_to_polylines("a?", 0.0, 0.0, 6.0, 0.0),
            Err(FontError::UnsupportedChar('?'))
        ));
    }

    #[test]
    fn anchor_and_rotation_apply() {
        // "I" rotated 90° CCW about (10, 20): the vertical stem becomes
        // horizontal, extending in −X from near the anchor.
        let polylines =
            text_to_polylines("I", 10.0, 20.0, 8.0, std::f64::consts::FRAC_PI_2).unwrap();
        for p in polylines.iter().flatten() {
            assert!(p.x <= 10.0 + 1e-9, "rotated glyph strayed +X: {p:?}");
            assert!(p.y >= 20.0 - 1e-9);
        }
    }

    #[test]
    fn text_width_matches_advance() {
        // "I" (2) + spacing (2) + "A" (6) = 10 grid units; height 8 → scale 1.
        assert!((text_width("IA", 8.0).unwrap() - 10.0).abs() < 1e-12);
        assert_eq!(text_width("", 8.0).unwrap(), 0.0);
    }
}
