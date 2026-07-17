//! Deterministic PDF 1.4 export for [`DrawingSheet`].
//!
//! Hand-rolled, dependency-free, fully vector output: line weights and dash
//! patterns come from each primitive's [`LineClass`], text uses the built-in
//! Helvetica base font. The byte stream contains no timestamps or random
//! IDs, so identical sheets produce identical files — which is what the
//! golden-file test suite relies on.

use std::fmt::Write as _;

use crate::dimension::{RenderedText, TextAlignment};
use crate::sheet::{DrawingSheet, LineClass};
use crate::types::Point2D;

/// Points per millimeter (PDF user space is 1/72 inch).
const PT_PER_MM: f64 = 72.0 / 25.4;

/// Approximate Helvetica advance width as a fraction of font size, used for
/// text alignment without embedding font metrics.
const AVG_CHAR_WIDTH: f64 = 0.55;

/// Angular step for arc flattening (radians, ~5°).
const ARC_STEP: f64 = std::f64::consts::PI / 36.0;

fn mm(v: f64) -> f64 {
    v * PT_PER_MM
}

fn fnum(v: f64) -> String {
    // Fixed-precision, canonical formatting keeps output deterministic.
    let s = format!("{v:.3}");
    // Normalize negative zero.
    if s == "-0.000" {
        "0.000".to_string()
    } else {
        s
    }
}

/// Escape a PDF literal string.
fn escape_pdf_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '(' => out.push_str("\\("),
            ')' => out.push_str("\\)"),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 128 => out.push(c),
            // Map common drafting symbols into WinAnsi where possible,
            // otherwise substitute.
            '⌀' | 'Ø' => out.push_str("\\330"), // Ø in WinAnsiEncoding
            '°' => out.push_str("\\260"),
            '±' => out.push_str("\\261"),
            _ => out.push('?'),
        }
    }
    out
}

/// Emit stroking commands for all primitives of one line class.
fn emit_class(content: &mut String, sheet: &DrawingSheet, class: LineClass) {
    let has_any = sheet.lines.iter().any(|l| l.class == class)
        || sheet.arcs.iter().any(|a| a.class == class)
        || sheet.polygons.iter().any(|p| p.class == class);
    if !has_any {
        return;
    }

    let _ = writeln!(content, "q");
    let _ = writeln!(content, "{} w", fnum(mm(class.weight_mm())));
    let dash = class.dash_pattern_mm();
    if dash.is_empty() {
        let _ = writeln!(content, "[] 0 d");
    } else {
        let pattern: Vec<String> = dash.iter().map(|d| fnum(mm(*d))).collect();
        let _ = writeln!(content, "[{}] 0 d", pattern.join(" "));
    }
    let _ = writeln!(content, "1 J 1 j");

    // Upstream mesh processing uses hash maps, so primitive order is not
    // stable across runs; sort canonically so output is byte-deterministic.
    let mut lines: Vec<_> = sheet.lines.iter().filter(|l| l.class == class).collect();
    lines.sort_by(|a, b| {
        (a.start.x, a.start.y, a.end.x, a.end.y)
            .partial_cmp(&(b.start.x, b.start.y, b.end.x, b.end.y))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for line in lines {
        let _ = writeln!(
            content,
            "{} {} m {} {} l S",
            fnum(mm(line.start.x)),
            fnum(mm(line.start.y)),
            fnum(mm(line.end.x)),
            fnum(mm(line.end.y)),
        );
    }

    let mut arcs: Vec<_> = sheet.arcs.iter().filter(|a| a.class == class).collect();
    arcs.sort_by(|a, b| {
        (
            a.arc.center.x,
            a.arc.center.y,
            a.arc.radius,
            a.arc.start_angle,
        )
            .partial_cmp(&(
                b.arc.center.x,
                b.arc.center.y,
                b.arc.radius,
                b.arc.start_angle,
            ))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for sheet_arc in arcs {
        let arc = &sheet_arc.arc;
        let span = arc.end_angle - arc.start_angle;
        let steps = ((span.abs() / ARC_STEP).ceil() as usize).max(2);
        for i in 0..=steps {
            let t = arc.start_angle + span * (i as f64) / (steps as f64);
            let p = Point2D::new(
                arc.center.x + arc.radius * t.cos(),
                arc.center.y + arc.radius * t.sin(),
            );
            let op = if i == 0 { "m" } else { "l" };
            let _ = writeln!(content, "{} {} {}", fnum(mm(p.x)), fnum(mm(p.y)), op);
        }
        let _ = writeln!(content, "S");
    }

    for poly in sheet.polygons.iter().filter(|p| p.class == class) {
        for (i, p) in poly.points.iter().enumerate() {
            let op = if i == 0 { "m" } else { "l" };
            let _ = writeln!(content, "{} {} {}", fnum(mm(p.x)), fnum(mm(p.y)), op);
        }
        let _ = writeln!(content, "f");
    }

    let _ = writeln!(content, "Q");
}

/// Horizontal/vertical anchor offsets for a text run, in text-space units of
/// (width, height).
fn alignment_offsets(alignment: TextAlignment) -> (f64, f64) {
    let h = match alignment {
        TextAlignment::TopLeft | TextAlignment::MiddleLeft | TextAlignment::BottomLeft => 0.0,
        TextAlignment::TopCenter | TextAlignment::MiddleCenter | TextAlignment::BottomCenter => {
            -0.5
        }
        TextAlignment::TopRight | TextAlignment::MiddleRight | TextAlignment::BottomRight => -1.0,
    };
    let v = match alignment {
        TextAlignment::TopLeft | TextAlignment::TopCenter | TextAlignment::TopRight => -1.0,
        TextAlignment::MiddleLeft | TextAlignment::MiddleCenter | TextAlignment::MiddleRight => {
            -0.5
        }
        TextAlignment::BottomLeft | TextAlignment::BottomCenter | TextAlignment::BottomRight => 0.0,
    };
    (h, v)
}

fn emit_text(content: &mut String, text: &RenderedText) {
    let size = mm(text.height) / 0.72; // cap height ≈ 0.72 em in Helvetica
    let width = text.text.chars().count() as f64 * size * AVG_CHAR_WIDTH;
    let (ho, vo) = alignment_offsets(text.alignment);
    // Anchor offset in unrotated text space.
    let ox = ho * width;
    let oy = vo * size * 0.72;

    let (c, s) = (text.rotation.cos(), text.rotation.sin());
    let x = mm(text.position.x) + ox * c - oy * s;
    let y = mm(text.position.y) + ox * s + oy * c;

    let _ = writeln!(content, "BT");
    let _ = writeln!(content, "/F1 {} Tf", fnum(size));
    let _ = writeln!(
        content,
        "{} {} {} {} {} {} Tm",
        fnum(c),
        fnum(s),
        fnum(-s),
        fnum(c),
        fnum(x),
        fnum(y)
    );
    let _ = writeln!(content, "({}) Tj", escape_pdf_text(&text.text));
    let _ = writeln!(content, "ET");
}

/// Render a [`DrawingSheet`] to PDF bytes.
///
/// Output is deterministic: the same sheet always produces byte-identical
/// PDF, making it safe to golden-test.
pub fn sheet_to_pdf(sheet: &DrawingSheet) -> Vec<u8> {
    let (w_mm, h_mm) = sheet.size.dimensions_mm();

    // Build the page content stream.
    let mut content = String::new();
    for class in [
        LineClass::Border,
        LineClass::Visible,
        LineClass::Hidden,
        LineClass::Section,
        LineClass::Hatch,
        LineClass::Dimension,
        LineClass::CuttingPlane,
        LineClass::Center,
    ] {
        emit_class(&mut content, sheet, class);
    }
    for text in &sheet.texts {
        emit_text(&mut content, text);
    }

    // Assemble the PDF file.
    let mut pdf: Vec<u8> = Vec::new();
    let mut offsets: Vec<usize> = Vec::new();
    pdf.extend_from_slice(b"%PDF-1.4\n");

    let push_obj = |pdf: &mut Vec<u8>, offsets: &mut Vec<usize>, body: String| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(body.as_bytes());
    };

    push_obj(
        &mut pdf,
        &mut offsets,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n".to_string(),
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n".to_string(),
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        format!(
            "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {} {}] \
             /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n",
            fnum(mm(w_mm)),
            fnum(mm(h_mm))
        ),
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica \
         /Encoding /WinAnsiEncoding >>\nendobj\n"
            .to_string(),
    );
    push_obj(
        &mut pdf,
        &mut offsets,
        format!(
            "5 0 obj\n<< /Length {} >>\nstream\n{}endstream\nendobj\n",
            content.len(),
            content
        ),
    );

    let xref_offset = pdf.len();
    let mut xref = String::new();
    let _ = writeln!(xref, "xref");
    let _ = writeln!(xref, "0 {}", offsets.len() + 1);
    let _ = writeln!(xref, "0000000000 65535 f ");
    for off in &offsets {
        let _ = writeln!(xref, "{off:010} 00000 n ");
    }
    let _ = writeln!(
        xref,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{}\n%%EOF",
        offsets.len() + 1,
        xref_offset
    );
    pdf.extend_from_slice(xref.as_bytes());

    pdf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sheet::{DrawingSheet, SheetSize, TitleBlock, TitleBlockFields};

    #[test]
    fn pdf_structure_valid() {
        let mut sheet = DrawingSheet::new(SheetSize::A4);
        sheet.add_title_block(&TitleBlock::new(TitleBlockFields {
            part_name: "TEST (PART)".into(),
            ..Default::default()
        }));
        let bytes = sheet_to_pdf(&sheet);
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.starts_with("%PDF-1.4"));
        assert!(text.trim_end().ends_with("%%EOF"));
        assert!(text.contains("/Type /Catalog"));
        assert!(text.contains("/BaseFont /Helvetica"));
        // Parens in text must be escaped.
        assert!(text.contains("TEST \\(PART\\)"));
        // Border weight 0.7 mm → 1.984 pt.
        assert!(text.contains("1.984 w"));
    }

    #[test]
    fn pdf_is_deterministic() {
        let build = || {
            let mut sheet = DrawingSheet::new(SheetSize::A3);
            sheet.add_title_block(&TitleBlock::new(TitleBlockFields::default()));
            sheet_to_pdf(&sheet)
        };
        assert_eq!(build(), build());
    }

    #[test]
    fn xref_offsets_are_correct() {
        let sheet = DrawingSheet::new(SheetSize::A4);
        let bytes = sheet_to_pdf(&sheet);
        let text = String::from_utf8_lossy(&bytes);
        // Each xref entry must point at "N 0 obj".
        let xref_start = text.find("xref\n").unwrap();
        for (i, line) in text[xref_start..]
            .lines()
            .skip(3) // "xref", "0 6", free entry
            .take(5)
            .enumerate()
        {
            let off: usize = line[..10].parse().unwrap();
            let expected = format!("{} 0 obj", i + 1);
            assert!(
                text[off..].starts_with(&expected),
                "xref entry {i} points at {:?}",
                &text[off..off + 12]
            );
        }
    }
}
