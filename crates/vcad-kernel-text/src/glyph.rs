//! Glyph outline extraction from fonts.

use ttf_parser::{Face, GlyphId, OutlineBuilder};
use vcad_kernel_sketch::SketchSegment;

/// Point in 2D font units.
#[derive(Debug, Clone, Copy)]
pub struct Point2D {
    pub x: f64,
    pub y: f64,
}

/// A contour (closed path) from a glyph outline.
#[derive(Debug, Clone)]
pub struct Contour {
    /// Points forming the contour (closed loop).
    pub points: Vec<Point2D>,
}

/// Builder that collects glyph outline into contours.
struct ContourBuilder {
    contours: Vec<Contour>,
    current_points: Vec<Point2D>,
    current_pos: Point2D,
}

impl ContourBuilder {
    fn new() -> Self {
        Self {
            contours: Vec::new(),
            current_points: Vec::new(),
            current_pos: Point2D { x: 0.0, y: 0.0 },
        }
    }

    fn finish(mut self) -> Vec<Contour> {
        // Close any open contour
        if !self.current_points.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current_points),
            });
        }
        self.contours
    }
}

impl OutlineBuilder for ContourBuilder {
    fn move_to(&mut self, x: f32, y: f32) {
        // Close previous contour if any
        if !self.current_points.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current_points),
            });
        }
        let pt = Point2D {
            x: x as f64,
            y: y as f64,
        };
        self.current_pos = pt;
        self.current_points.push(pt);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let pt = Point2D {
            x: x as f64,
            y: y as f64,
        };
        self.current_pos = pt;
        self.current_points.push(pt);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // Approximate quadratic bezier with line segments
        // Using 4 subdivisions for reasonable quality
        let p0 = self.current_pos;
        let p1 = Point2D {
            x: x1 as f64,
            y: y1 as f64,
        };
        let p2 = Point2D {
            x: x as f64,
            y: y as f64,
        };

        const SUBDIVISIONS: usize = 4;
        for i in 1..=SUBDIVISIONS {
            let t = i as f64 / SUBDIVISIONS as f64;
            let t2 = t * t;
            let mt = 1.0 - t;
            let mt2 = mt * mt;

            let pt = Point2D {
                x: mt2 * p0.x + 2.0 * mt * t * p1.x + t2 * p2.x,
                y: mt2 * p0.y + 2.0 * mt * t * p1.y + t2 * p2.y,
            };
            self.current_points.push(pt);
        }
        self.current_pos = p2;
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        // Approximate cubic bezier with line segments
        // Using 8 subdivisions for higher quality curves
        let p0 = self.current_pos;
        let p1 = Point2D {
            x: x1 as f64,
            y: y1 as f64,
        };
        let p2 = Point2D {
            x: x2 as f64,
            y: y2 as f64,
        };
        let p3 = Point2D {
            x: x as f64,
            y: y as f64,
        };

        const SUBDIVISIONS: usize = 8;
        for i in 1..=SUBDIVISIONS {
            let t = i as f64 / SUBDIVISIONS as f64;
            let t2 = t * t;
            let t3 = t2 * t;
            let mt = 1.0 - t;
            let mt2 = mt * mt;
            let mt3 = mt2 * mt;

            let pt = Point2D {
                x: mt3 * p0.x + 3.0 * mt2 * t * p1.x + 3.0 * mt * t2 * p2.x + t3 * p3.x,
                y: mt3 * p0.y + 3.0 * mt2 * t * p1.y + 3.0 * mt * t2 * p2.y + t3 * p3.y,
            };
            self.current_points.push(pt);
        }
        self.current_pos = p3;
    }

    fn close(&mut self) {
        // Close the contour
        if !self.current_points.is_empty() {
            self.contours.push(Contour {
                points: std::mem::take(&mut self.current_points),
            });
        }
    }
}

/// Extract contours from a glyph.
pub fn extract_glyph_contours(face: &Face<'_>, glyph_id: GlyphId) -> Vec<Contour> {
    let mut builder = ContourBuilder::new();
    face.outline_glyph(glyph_id, &mut builder);
    builder.finish()
}

/// Convert a contour to sketch segments.
///
/// Returns segments forming a closed loop.
pub fn contour_to_segments(
    contour: &Contour,
    scale: f64,
    offset_x: f64,
    offset_y: f64,
) -> Vec<SketchSegment> {
    use vcad_kernel_sketch::SketchSegment as Seg;

    if contour.points.len() < 2 {
        return Vec::new();
    }

    let mut segments = Vec::with_capacity(contour.points.len());

    for i in 0..contour.points.len() {
        let p0 = &contour.points[i];
        let p1 = &contour.points[(i + 1) % contour.points.len()];

        let start = vcad_kernel_sketch::SketchSegment::Line {
            start: vcad_kernel_math::Point2::new(p0.x * scale + offset_x, p0.y * scale + offset_y),
            end: vcad_kernel_math::Point2::new(p1.x * scale + offset_x, p1.y * scale + offset_y),
        };

        // Skip degenerate segments
        if let Seg::Line { start: s, end: e } = &start {
            let len = ((e.x - s.x).powi(2) + (e.y - s.y).powi(2)).sqrt();
            if len > 1e-6 {
                segments.push(start);
            }
        }
    }

    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::FontRegistry;

    #[test]
    fn test_extract_glyph_contours() {
        let font = FontRegistry::builtin_sans();
        let face = font.face();
        let glyph_id = face.glyph_index('O').unwrap();
        let contours = extract_glyph_contours(&face, glyph_id);

        // 'O' typically has 2 contours: outer and inner (hole)
        assert!(contours.len() >= 1);
        assert!(!contours[0].points.is_empty());
    }

    #[test]
    fn test_contour_to_segments() {
        let contour = Contour {
            points: vec![
                Point2D { x: 0.0, y: 0.0 },
                Point2D { x: 100.0, y: 0.0 },
                Point2D { x: 100.0, y: 100.0 },
                Point2D { x: 0.0, y: 100.0 },
            ],
        };

        let segments = contour_to_segments(&contour, 0.01, 0.0, 0.0);
        assert_eq!(segments.len(), 4);
    }
}
