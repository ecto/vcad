//! Multi-view drawing sheets: front, side, top, and isometric views laid
//! out in the classic third-angle arrangement with a title block.
//!
//! This module is a *composition layer*. It renders each view through the
//! crate's ordinary public entry points ([`render_svg_str_opts`] /
//! [`render_png_str`]) at one shared scale and arranges the results — it
//! deliberately owns none of the projection, shading, or hidden-line logic.
//! Keeping it decoupled means new single-view features (sections, exact
//! edges, ray tracing, annotations) neither need nor get special-casing
//! here.
//!
//! All four views share one pixels-per-millimetre scale, so the sheet is
//! dimensionally consistent: a feature that measures 10mm reads the same
//! length in every view.

use crate::{evaluate_vcad, render_svg_str_opts, SvgOptions, View, INK, PADDING_PX, PAPER};

/// Landscape drawing-sheet aspect ratio (A-series: height = width / √2).
const SHEET_ASPECT: f64 = 0.707;

/// The classic third-angle arrangement: top view above the front view, side
/// view to the right of front, isometric in the remaining (top-right)
/// corner. `(view, caption, column, row)`.
const SHEET_VIEWS: [(View, &str, usize, usize); 4] = [
    (View::Top, "TOP", 0, 0),
    (View::Isometric, "ISO", 1, 0),
    (View::Front, "FRONT", 0, 1),
    (View::Side, "SIDE", 1, 1),
];

/// Options for [`render_sheet_svg_str`].
#[derive(Debug, Clone)]
pub struct SheetOptions {
    /// Overall sheet width in user units / pixels; height is derived
    /// (landscape, [`SHEET_ASPECT`]).
    pub width_px: f64,
    /// Title shown in the title block (typically the document/file name).
    pub title: String,
}

impl Default for SheetOptions {
    fn default() -> Self {
        SheetOptions {
            width_px: 1600.0,
            title: "UNTITLED".to_string(),
        }
    }
}

/// Everything positional about a sheet: the shared scale, the 2×2 view
/// grid, and the title block. Computed once from the model's 3D bbox and
/// shared by the SVG and raster composers so both lay out identically.
struct SheetLayout {
    w: f64,
    h: f64,
    margin: f64,
    caption_h: f64,
    /// Shared pixels-per-millimetre — every view uses this one scale so the
    /// four projections are dimensionally consistent.
    scale: f64,
    cell: f64,
    cell_x: [f64; 2],
    cell_y: [f64; 2],
    /// Title block `(x, y, w, h)`.
    tb: (f64, f64, f64, f64),
    /// Overall model dimensions `[dx, dy, dz]` in mm.
    dims: [f64; 3],
}

/// Projected screen-plane extent `(width, height)` of the 3D bbox in
/// `view`. Projection is linear, so the projected model always fits inside
/// the projected bbox — a safe (for iso, slightly conservative) fit bound.
fn view_extent(view: View, lo: [f64; 3], hi: [f64; 3]) -> (f64, f64) {
    let right = view.right();
    let down = view.down();
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let mut min = [f64::INFINITY; 2];
    let mut max = [f64::NEG_INFINITY; 2];
    for i in 0..8u32 {
        let p = [
            if i & 1 == 0 { lo[0] } else { hi[0] },
            if i & 2 == 0 { lo[1] } else { hi[1] },
            if i & 4 == 0 { lo[2] } else { hi[2] },
        ];
        let s = [dot(p, right), dot(p, down)];
        for a in 0..2 {
            min[a] = min[a].min(s[a]);
            max[a] = max[a].max(s[a]);
        }
    }
    (max[0] - min[0], max[1] - min[1])
}

/// The document's overall 3D bounding box (coarse tessellation — plenty for
/// layout; each view render tessellates finely itself).
fn document_bbox(raw_vcad: &str) -> Result<([f64; 3], [f64; 3]), String> {
    let scene = evaluate_vcad(raw_vcad)?;
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for s in &scene {
        let mesh = s.solid.to_mesh(32);
        for v in mesh.vertices.chunks(3) {
            if v.len() < 3 {
                continue;
            }
            for i in 0..3 {
                lo[i] = lo[i].min(v[i] as f64);
                hi[i] = hi[i].max(v[i] as f64);
            }
        }
    }
    if lo[0] > hi[0] {
        return Err("no solids produced".to_string());
    }
    Ok((lo, hi))
}

fn sheet_layout(width_px: f64, lo: [f64; 3], hi: [f64; 3]) -> Result<SheetLayout, String> {
    let w = width_px;
    let h = w * SHEET_ASPECT;
    let margin = w * 0.03;
    let gutter = w * 0.025;
    let caption_h = (w * 0.014).clamp(9.0, 24.0);
    let tb_h = (h * 0.14).clamp(48.0, 140.0);
    let grid_w = w - 2.0 * margin;
    let grid_h = h - 2.0 * margin - tb_h - gutter;
    // Each view occupies a column × row band; the view itself renders into a
    // square cell centred in its band (the raster path renders each view
    // into a square canvas, and one shape keeps the SVG and raster sheets
    // laid out identically).
    let col_w = (grid_w - gutter) / 2.0;
    let band_h = (grid_h - gutter) / 2.0;
    let cell = col_w.min(band_h - caption_h);
    if cell <= 4.0 * PADDING_PX {
        return Err("sheet size too small for a 2x2 view grid".to_string());
    }
    // One shared scale: the largest px/mm at which every view still fits
    // its cell (leaving room for the per-view padding).
    let avail = cell - 2.0 * PADDING_PX;
    let mut scale = f64::INFINITY;
    for (view, _, _, _) in SHEET_VIEWS {
        let (ew, eh) = view_extent(view, lo, hi);
        if ew > 1e-9 {
            scale = scale.min(avail / ew);
        }
        if eh > 1e-9 {
            scale = scale.min(avail / eh);
        }
    }
    if !scale.is_finite() || scale <= 0.0 {
        return Err("degenerate model extents".to_string());
    }
    let tb_w = (w * 0.34).min(grid_w);
    // Centre each square cell in its column/row band so the grid reads
    // balanced across the sheet rather than hugging the top-left.
    let cell_pad_x = (col_w - cell) / 2.0;
    let cell_pad_y = (band_h - caption_h - cell) / 2.0;
    Ok(SheetLayout {
        w,
        h,
        margin,
        caption_h,
        scale,
        cell,
        cell_x: [margin + cell_pad_x, margin + col_w + gutter + cell_pad_x],
        cell_y: [margin + cell_pad_y, margin + band_h + gutter + cell_pad_y],
        tb: (w - margin - tb_w, h - margin - tb_h, tb_w, tb_h),
        dims: [hi[0] - lo[0], hi[1] - lo[1], hi[2] - lo[2]],
    })
}

/// The four text lines of the title block (title first, then dimensions,
/// scale, and a date placeholder to be filled in by hand).
fn title_block_lines(title: &str, lay: &SheetLayout) -> [String; 4] {
    [
        title.to_ascii_uppercase(),
        format!(
            "{:.1} X {:.1} X {:.1} MM",
            lay.dims[0], lay.dims[1], lay.dims[2]
        ),
        format!("SCALE {:.2} PX/MM", lay.scale),
        "DATE ____-__-__".to_string(),
    ]
}

/// Cap a text height so the run fits within `max_w` (stroke-font advance
/// scales linearly with cap height).
fn fit_text_height(text: &str, height: f64, max_w: f64) -> f64 {
    let unit_w = vcad_ir::stroke_font::text_width(text, 1.0);
    if unit_w <= 0.0 {
        return height;
    }
    height.min(max_w / unit_w)
}

/// Read a numeric attribute out of an SVG opening tag.
fn svg_attr(svg: &str, name: &str) -> Option<f64> {
    let pat = format!("{name}=\"");
    let start = svg.find(&pat)? + pat.len();
    let end = start + svg[start..].find('"')?;
    svg[start..end].parse().ok()
}

/// Namespace every `id="…"` (and its `url(#…)` references) in a rendered
/// view so several views can be nested in one sheet document without id
/// collisions — SVG ids are document-global, so two views both defining
/// `g0` would otherwise share one gradient.
fn namespace_ids(svg: &str, prefix: &str) -> String {
    svg.replace("id=\"", &format!("id=\"{prefix}"))
        .replace("url(#", &format!("url(#{prefix}"))
}

/// Emit single-stroke text (the shared drafting font) as SVG paths.
/// `x` is the left edge, or the centre when `centered`; `baseline` is the
/// text baseline in SVG coords (y grows down).
fn push_sheet_text(
    out: &mut String,
    text: &str,
    x: f64,
    baseline: f64,
    height: f64,
    centered: bool,
) {
    let strokes = vcad_ir::stroke_font::text_strokes(text, height);
    if strokes.is_empty() {
        return;
    }
    let x0 = if centered {
        x - vcad_ir::stroke_font::text_width(text, height) / 2.0
    } else {
        x
    };
    let sw = (height * 0.09).clamp(0.6, 2.0);
    out.push_str(&format!(
        r#"<g stroke="{INK}" stroke-width="{sw:.2}" stroke-linecap="round" stroke-linejoin="round" fill="none">"#
    ));
    for stroke in &strokes {
        let mut d = String::new();
        for (i, &(lx, ly)) in stroke.iter().enumerate() {
            let cmd = if i == 0 { 'M' } else { 'L' };
            d.push_str(&format!("{cmd}{:.2} {:.2}", x0 + lx, baseline - ly));
        }
        out.push_str(&format!(r#"<path d="{d}"/>"#));
    }
    out.push_str("</g>");
}

/// Minimal XML attribute escaping for user-supplied strings.
fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Append the title block: a bordered box with the document name, overall
/// bounding-box dimensions, the shared scale, and a date placeholder.
fn push_title_block(body: &mut String, lay: &SheetLayout, title: &str) {
    let (tx, ty, tw, th) = lay.tb;
    let lines = title_block_lines(title, lay);
    body.push_str(&format!(
        r#"<g class="title-block" data-title="{}" data-dims="{}">"#,
        xml_escape(title),
        xml_escape(&lines[1]),
    ));
    body.push_str(&format!(
        r#"<rect x="{tx:.2}" y="{ty:.2}" width="{tw:.2}" height="{th:.2}" fill="none" stroke="{INK}" stroke-width="1.2"/>"#
    ));
    let pad = tw * 0.05;
    let title_h = (th * 0.22).clamp(8.0, 30.0);
    let line_h = (th * 0.14).clamp(6.0, 20.0);
    let rule_y = ty + th * 0.34;
    body.push_str(&format!(
        r#"<line x1="{tx:.2}" y1="{rule_y:.2}" x2="{:.2}" y2="{rule_y:.2}" stroke="{INK}" stroke-width="0.6"/>"#,
        tx + tw,
    ));
    let fit_w = tw - 2.0 * pad;
    push_sheet_text(
        body,
        &lines[0],
        tx + pad,
        ty + th * 0.26,
        fit_text_height(&lines[0], title_h, fit_w),
        false,
    );
    for (i, line) in lines[1..].iter().enumerate() {
        let baseline = rule_y + th * 0.2 * (i as f64 + 1.0);
        push_sheet_text(
            body,
            line,
            tx + pad,
            baseline,
            fit_text_height(line, line_h, fit_w),
            false,
        );
    }
    body.push_str("</g>");
}

/// Render raw `.vcad` document JSON to a multi-view drawing sheet SVG:
/// front, side, top, and isometric views in third-angle arrangement, all at
/// one shared scale, with view captions, a thin border frame, and a title
/// block (title, overall dimensions, scale, date placeholder).
///
/// Each view is rendered through [`render_svg_str_opts`] and nested in the
/// sheet as its own `<svg>` viewport, so views stay pixel-identical to the
/// equivalent single-view render.
pub fn render_sheet_svg_str(raw_vcad: &str, opts: &SheetOptions) -> Result<String, String> {
    let (lo, hi) = document_bbox(raw_vcad)?;
    let lay = sheet_layout(opts.width_px, lo, hi)?;

    let mut body = String::new();
    for (idx, (view, caption, col, row)) in SHEET_VIEWS.iter().enumerate() {
        // Transparent: the sheet lays down its own vellum ground, and the
        // views must not paint opaque rects over each other's cells.
        let view_svg = render_svg_str_opts(
            raw_vcad,
            lay.scale,
            &SvgOptions {
                view: *view,
                transparent: true,
                ..Default::default()
            },
        )?;
        let vw = svg_attr(&view_svg, "width").unwrap_or(lay.cell);
        let vh = svg_attr(&view_svg, "height").unwrap_or(lay.cell);
        let cx = lay.cell_x[*col];
        let cy = lay.cell_y[*row];
        // Centre the view in its cell.
        let gx = cx + (lay.cell - vw) / 2.0;
        let gy = cy + (lay.cell - vh) / 2.0;
        body.push_str(&format!(
            r#"<g class="sheet-view" data-view="{}" transform="translate({gx:.2} {gy:.2})">{}</g>"#,
            caption.to_ascii_lowercase(),
            namespace_ids(&view_svg, &format!("v{idx}")),
        ));
        push_sheet_text(
            &mut body,
            caption,
            cx + lay.cell / 2.0,
            cy + lay.cell + lay.caption_h,
            lay.caption_h * 0.66,
            true,
        );
    }
    push_title_block(&mut body, &lay, &opts.title);

    // Assemble: paper ground, thin border frame, views, title block.
    let (w, h) = (lay.w, lay.h);
    let frame = lay.margin * 0.5;
    let mut out = String::new();
    out.push_str(&format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.2}" height="{h:.2}" viewBox="0 0 {w:.2} {h:.2}" role="img" aria-label="vcad drawing sheet: {}">"#,
        xml_escape(&opts.title),
    ));
    out.push_str(&format!(
        r#"<rect x="0" y="0" width="{w:.2}" height="{h:.2}" fill="{PAPER}"/>"#
    ));
    out.push_str(&format!(
        r#"<rect x="{frame:.2}" y="{frame:.2}" width="{:.2}" height="{:.2}" fill="none" stroke="{INK}" stroke-width="1.2"/>"#,
        w - 2.0 * frame,
        h - 2.0 * frame,
    ));
    out.push_str(&body);
    out.push_str("</svg>");
    Ok(out)
}

#[cfg(feature = "raster")]
pub use raster_sheet::{render_sheet_jpeg_str, SheetRasterOptions};

#[cfg(feature = "raster")]
mod raster_sheet {
    use super::*;
    use crate::{render_png_str, RasterOptions, FILL_DARK, PAPER_RGB};

    /// Options for [`render_sheet_jpeg_str`].
    #[derive(Debug, Clone)]
    pub struct SheetRasterOptions {
        /// Overall sheet width in pixels; height is derived (landscape).
        pub width_px: u32,
        /// JPEG quality, 1–100.
        pub quality: u8,
        /// Title shown in the title block.
        pub title: String,
    }

    impl Default for SheetRasterOptions {
        fn default() -> Self {
            SheetRasterOptions {
                width_px: 1600,
                quality: 92,
                title: "UNTITLED".to_string(),
            }
        }
    }

    /// Paint a plain 2D line — sheet frame, captions, title-block linework.
    fn draw_line(rgb: &mut [u8], cw: usize, ch: usize, a: (f64, f64), b: (f64, f64)) {
        let len = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let steps = (len * 2.0).ceil().max(1.0) as usize;
        for s in 0..=steps {
            let t = s as f64 / steps as f64;
            let ix = (a.0 + (b.0 - a.0) * t).floor() as i64;
            let iy = (a.1 + (b.1 - a.1) * t).floor() as i64;
            for (dx, dy) in [(0i64, 0i64), (1, 0), (0, 1), (1, 1)] {
                let (qx, qy) = (ix + dx, iy + dy);
                if qx < 0 || qy < 0 || qx >= cw as i64 || qy >= ch as i64 {
                    continue;
                }
                let qi = (qy as usize * cw + qx as usize) * 3;
                rgb[qi..qi + 3].copy_from_slice(&FILL_DARK);
            }
        }
    }

    /// Draw single-stroke text into the canvas. `at` is `(x, baseline)`;
    /// `x` is the left edge, or the centre when `centered`.
    fn draw_text(
        rgb: &mut [u8],
        cw: usize,
        ch: usize,
        text: &str,
        at: (f64, f64),
        height: f64,
        centered: bool,
    ) {
        let (x, baseline) = at;
        let x0 = if centered {
            x - vcad_ir::stroke_font::text_width(text, height) / 2.0
        } else {
            x
        };
        for stroke in &vcad_ir::stroke_font::text_strokes(text, height) {
            for pair in stroke.windows(2) {
                draw_line(
                    rgb,
                    cw,
                    ch,
                    (x0 + pair[0].0, baseline - pair[0].1),
                    (x0 + pair[1].0, baseline - pair[1].1),
                );
            }
        }
    }

    /// Raster counterpart of [`render_sheet_svg_str`]: the same third-angle
    /// layout, shared scale, captions, frame, and title block, composed as a
    /// JPEG.
    ///
    /// Each view is rendered through [`render_png_str`] into a square cell
    /// canvas (transparent background) and alpha-composited onto the sheet.
    /// The shared scale is held by solving each view's `fill_frac` for the
    /// same pixels-per-millimetre.
    pub fn render_sheet_jpeg_str(
        raw_vcad: &str,
        opts: &SheetRasterOptions,
    ) -> Result<Vec<u8>, String> {
        if opts.width_px < 320 {
            return Err("sheet width_px too small".to_string());
        }
        let (lo, hi) = document_bbox(raw_vcad)?;
        let lay = sheet_layout(opts.width_px as f64, lo, hi)?;

        let w = lay.w.round() as usize;
        let h = lay.h.round() as usize;
        let mut rgb: Vec<u8> = PAPER_RGB.iter().copied().cycle().take(w * h * 3).collect();

        let cell = lay.cell.round() as u32;
        for (view, caption, col, row) in SHEET_VIEWS {
            // `render_png_str` derives px/mm as `fill_frac * size_px /
            // extent`; solve for the fill_frac that lands on the sheet's
            // one shared scale.
            let (ew, eh) = view_extent(view, lo, hi);
            let extent = ew.max(eh);
            if extent < 1e-9 {
                continue;
            }
            let fill_frac = (lay.scale * extent / cell as f64).clamp(0.01, 1.0);
            let png = render_png_str(
                raw_vcad,
                &RasterOptions {
                    view,
                    size_px: cell,
                    fill_frac,
                    quality: opts.quality,
                    ..Default::default()
                },
            )?;
            let img = image::load_from_memory(&png)
                .map_err(|e| format!("decode view png: {e}"))?
                .to_rgba8();
            let ox = lay.cell_x[col].round() as usize;
            let oy = lay.cell_y[row].round() as usize;
            // Alpha-composite the cell onto the vellum sheet.
            for (px, py, p) in img.enumerate_pixels() {
                let (sx, sy) = (ox + px as usize, oy + py as usize);
                if sx >= w || sy >= h {
                    continue;
                }
                let a = p[3] as f64 / 255.0;
                if a <= 0.0 {
                    continue;
                }
                let di = (sy * w + sx) * 3;
                for c in 0..3 {
                    let dst = rgb[di + c] as f64;
                    rgb[di + c] = (p[c] as f64 * a + dst * (1.0 - a)).round() as u8;
                }
            }
            draw_text(
                &mut rgb,
                w,
                h,
                caption,
                (
                    lay.cell_x[col] + lay.cell / 2.0,
                    lay.cell_y[row] + lay.cell + lay.caption_h,
                ),
                lay.caption_h * 0.66,
                true,
            );
        }

        // Thin border frame.
        let f = lay.margin * 0.5;
        for (a, b) in [
            ((f, f), (lay.w - f, f)),
            ((lay.w - f, f), (lay.w - f, lay.h - f)),
            ((lay.w - f, lay.h - f), (f, lay.h - f)),
            ((f, lay.h - f), (f, f)),
        ] {
            draw_line(&mut rgb, w, h, a, b);
        }

        // Title block.
        let (tx, ty, tw, th) = lay.tb;
        for (a, b) in [
            ((tx, ty), (tx + tw, ty)),
            ((tx + tw, ty), (tx + tw, ty + th)),
            ((tx + tw, ty + th), (tx, ty + th)),
            ((tx, ty + th), (tx, ty)),
            ((tx, ty + th * 0.34), (tx + tw, ty + th * 0.34)),
        ] {
            draw_line(&mut rgb, w, h, a, b);
        }
        let lines = title_block_lines(&opts.title, &lay);
        let pad = tw * 0.05;
        let title_h = (th * 0.22).clamp(8.0, 30.0);
        let line_h = (th * 0.14).clamp(6.0, 20.0);
        let fit_w = tw - 2.0 * pad;
        draw_text(
            &mut rgb,
            w,
            h,
            &lines[0],
            (tx + pad, ty + th * 0.26),
            fit_text_height(&lines[0], title_h, fit_w),
            false,
        );
        for (i, line) in lines[1..].iter().enumerate() {
            let baseline = ty + th * 0.34 + th * 0.2 * (i as f64 + 1.0);
            draw_text(
                &mut rgb,
                w,
                h,
                line,
                (tx + pad, baseline),
                fit_text_height(line, line_h, fit_w),
                false,
            );
        }

        let mut out = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(
            &mut out,
            opts.quality.clamp(1, 100),
        );
        enc.encode(&rgb, w as u32, h as u32, image::ExtendedColorType::Rgb8)
            .map_err(|e| format!("jpeg encode: {e}"))?;
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube_vcad(sx: f64, sy: f64, sz: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cube",
      "op": {{ "type": "Cube", "size": {{ "x": {sx}, "y": {sy}, "z": {sz} }} }}
    }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "default" }}]
}}"#
        )
    }

    /// The drawing sheet lays out all four views (third-angle) plus a title
    /// block carrying the document name and overall dimensions.
    #[test]
    fn sheet_svg_has_four_views_and_title_block() {
        let svg = render_sheet_svg_str(
            &cube_vcad(30.0, 20.0, 10.0),
            &SheetOptions {
                width_px: 900.0,
                title: "widget".to_string(),
            },
        )
        .expect("sheet should render");
        assert!(svg.starts_with("<svg "));
        assert!(svg.ends_with("</svg>"));
        for v in ["front", "side", "top", "iso"] {
            assert!(
                svg.contains(&format!(r#"data-view="{v}""#)),
                "missing {v} view group"
            );
        }
        assert_eq!(svg.matches(r#"class="sheet-view""#).count(), 4);
        assert!(svg.contains(r#"class="title-block""#));
        assert!(svg.contains(r#"data-title="widget""#));
        assert!(
            svg.contains(r#"data-dims="30.0 X 20.0 X 10.0 MM""#),
            "title block should carry overall bbox dimensions"
        );
        // Captions and title-block text are single-stroke paths.
        assert!(svg.contains("<path d=\"M"));
    }

    /// A cylinder: curved facets get Gouraud gradients, so this fixture
    /// actually produces the `id="gN"` defs the sheet must namespace.
    fn cylinder_vcad(radius: f64, height: f64) -> String {
        format!(
            r#"{{
  "version": "0.1",
  "nodes": {{
    "1": {{
      "id": 1,
      "name": "Cyl",
      "op": {{ "type": "Cylinder", "radius": {radius}, "height": {height}, "segments": 0 }}
    }}
  }},
  "materials": {{}},
  "part_materials": {{}},
  "roots": [{{ "root": 1, "material": "default" }}]
}}"#
        )
    }

    /// Nested view SVGs must not share gradient ids — SVG ids are
    /// document-global, so two views both defining `g0` would make one
    /// view's facets sample the other view's gradient.
    #[test]
    fn sheet_namespaces_view_gradient_ids() {
        let doc = cylinder_vcad(12.0, 25.0);
        // Sanity: a single view of this fixture really does emit gradients,
        // otherwise the assertions below would pass vacuously.
        let single = render_svg_str_opts(&doc, 4.0, &SvgOptions::default()).unwrap();
        assert!(
            single.contains(r#"id="g0""#),
            "fixture should emit gradient defs"
        );

        let svg = render_sheet_svg_str(
            &doc,
            &SheetOptions {
                width_px: 900.0,
                title: "t".to_string(),
            },
        )
        .unwrap();
        // Every id in a nested view carries its view's prefix; no bare
        // `id="g0"` survives to collide across views. (Which views emit
        // gradients depends on the geometry — a cylinder's TOP view is a
        // flat circle — so assert on the prefix, not a specific view.)
        assert!(!svg.contains(r#"id="g0""#), "un-namespaced gradient id");
        assert!(
            (0..SHEET_VIEWS.len()).any(|i| svg.contains(&format!(r#"id="v{i}g0""#))),
            "expected namespaced view ids"
        );
        // Every gradient reference resolves to a namespaced id.
        assert!(!svg.contains("url(#g"), "un-namespaced gradient reference");
    }

    #[test]
    fn rejects_garbage_input() {
        let err = render_sheet_svg_str("not json", &SheetOptions::default()).unwrap_err();
        assert!(err.starts_with("parse:"), "got: {err}");
    }

    #[cfg(feature = "raster")]
    #[test]
    fn renders_sheet_to_jpeg() {
        let jpg = render_sheet_jpeg_str(
            &cube_vcad(30.0, 20.0, 10.0),
            &SheetRasterOptions {
                width_px: 640,
                quality: 85,
                title: "widget".to_string(),
            },
        )
        .expect("sheet jpeg should render");
        assert!(jpg.len() > 1000);
        assert_eq!(&jpg[..2], &[0xFF, 0xD8], "missing JPEG SOI marker");
    }
}
