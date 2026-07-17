//! Drawing-sheet composition: line classes, title block, revision table,
//! BOM balloons/table, and a sheet model that places views and annotations
//! on a bordered page ready for PDF or DXF output.
//!
//! Everything here renders down to the same primitive vocabulary as
//! dimensions ([`RenderedDimension`]: lines, arcs, arrows, texts), tagged
//! with a [`LineClass`] so exporters can apply standard weights and dash
//! patterns.

use serde::{Deserialize, Serialize};

use crate::dimension::{
    ArrowType, DimensionStyle, RenderedArc, RenderedDimension, RenderedText, TextAlignment,
};
use crate::types::{BoundingBox2D, DetailView, Point2D, ProjectedView, SectionView, Visibility};

// ============================================================================
// Line classes (weights + dash patterns per ANSI/ISO)
// ============================================================================

/// Drafting line classification, mapping to standard pen weights and dash
/// patterns (ANSI Y14.2 / ISO 128).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineClass {
    /// Visible object lines — thick, continuous.
    Visible,
    /// Hidden lines — thin, dashed.
    Hidden,
    /// Section cut outlines — thick, continuous.
    Section,
    /// Cross-hatch lines — thin, continuous.
    Hatch,
    /// Dimension, extension, and leader lines — thin, continuous.
    Dimension,
    /// Cutting-plane (phantom) lines — thick, dash-dot.
    CuttingPlane,
    /// Centerlines — thin, dash-dot.
    Center,
    /// Sheet border and table frames — extra thick, continuous.
    Border,
}

impl LineClass {
    /// Standard pen weight in millimeters.
    pub fn weight_mm(&self) -> f64 {
        match self {
            LineClass::Visible | LineClass::Section => 0.5,
            LineClass::Hidden => 0.35,
            LineClass::Hatch | LineClass::Dimension => 0.25,
            LineClass::CuttingPlane => 0.6,
            LineClass::Center => 0.25,
            LineClass::Border => 0.7,
        }
    }

    /// Dash pattern in millimeters (`on, off, on, off, …`), empty = continuous.
    pub fn dash_pattern_mm(&self) -> &'static [f64] {
        match self {
            LineClass::Hidden => &[3.0, 1.5],
            LineClass::CuttingPlane => &[8.0, 1.5, 1.5, 1.5],
            LineClass::Center => &[6.0, 1.5, 1.0, 1.5],
            _ => &[],
        }
    }

    /// DXF layer name for this class.
    pub fn dxf_layer(&self) -> &'static str {
        match self {
            LineClass::Visible => "VISIBLE",
            LineClass::Hidden => "HIDDEN",
            LineClass::Section => "SECTION",
            LineClass::Hatch => "HATCH",
            LineClass::Dimension => "DIMENSION",
            LineClass::CuttingPlane => "CUTPLANE",
            LineClass::Center => "CENTER",
            LineClass::Border => "BORDER",
        }
    }
}

// ============================================================================
// Title block
// ============================================================================

/// Parametric fields of a title block.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TitleBlockFields {
    /// Part or assembly name.
    pub part_name: String,
    /// Material specification (e.g. "6061-T6 AL").
    pub material: String,
    /// Surface finish note (e.g. "ANODIZE CLR").
    pub finish: String,
    /// Drawing scale (e.g. "1:1").
    pub scale: String,
    /// Author.
    pub drawn_by: String,
    /// Date string (caller-formatted, e.g. "2026-07-17").
    pub date: String,
    /// Current revision letter (e.g. "A").
    pub revision: String,
    /// Units note (defaults rendered as-is, e.g. "MM").
    pub units: String,
    /// General tolerance note (e.g. "±0.1 UNLESS NOTED").
    pub tolerance_note: String,
}

/// A title block entity: a bordered grid of labeled fields, rendered at a
/// given origin (bottom-left corner of the block).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleBlock {
    /// Field values.
    pub fields: TitleBlockFields,
    /// Total block width in mm.
    pub width: f64,
    /// Total block height in mm.
    pub height: f64,
}

/// Draw one labeled cell: frame lines + small label + centered value.
fn labeled_cell(
    rd: &mut RenderedDimension,
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    label: &str,
    value: &str,
) {
    rd.add_line(Point2D::new(x, y), Point2D::new(x + w, y));
    rd.add_line(Point2D::new(x + w, y), Point2D::new(x + w, y + h));
    rd.add_line(Point2D::new(x + w, y + h), Point2D::new(x, y + h));
    rd.add_line(Point2D::new(x, y + h), Point2D::new(x, y));
    rd.add_text(
        RenderedText::new(Point2D::new(x + 1.0, y + h - 1.0), label, 1.8)
            .with_alignment(TextAlignment::TopLeft),
    );
    rd.add_text(
        RenderedText::new(Point2D::new(x + w / 2.0, y + h / 2.0 - 1.0), value, 3.0)
            .with_alignment(TextAlignment::MiddleCenter),
    );
}

impl TitleBlock {
    /// Create a title block with the standard 180×36 mm footprint.
    pub fn new(fields: TitleBlockFields) -> Self {
        Self {
            fields,
            width: 180.0,
            height: 36.0,
        }
    }

    /// Render the title block with its bottom-left corner at `origin`.
    ///
    /// Layout: full-width part-name band on top, then two rows of four
    /// cells (material / finish / tolerance / units, then scale / drawn-by /
    /// date / rev).
    pub fn render(&self, origin: Point2D) -> RenderedDimension {
        let mut rd = RenderedDimension::new();
        let row_h = self.height / 3.0;
        let f = &self.fields;

        // Top band: part name.
        labeled_cell(
            &mut rd,
            origin.x,
            origin.y + 2.0 * row_h,
            self.width,
            row_h,
            "PART",
            &f.part_name,
        );

        let col_w = self.width / 4.0;
        let mid: [(&str, &str); 4] = [
            ("MATERIAL", f.material.as_str()),
            ("FINISH", f.finish.as_str()),
            ("TOLERANCE", f.tolerance_note.as_str()),
            ("UNITS", f.units.as_str()),
        ];
        for (i, (label, value)) in mid.iter().enumerate() {
            labeled_cell(
                &mut rd,
                origin.x + i as f64 * col_w,
                origin.y + row_h,
                col_w,
                row_h,
                label,
                value,
            );
        }

        let bottom: [(&str, &str); 4] = [
            ("SCALE", f.scale.as_str()),
            ("DRAWN BY", f.drawn_by.as_str()),
            ("DATE", f.date.as_str()),
            ("REV", f.revision.as_str()),
        ];
        for (i, (label, value)) in bottom.iter().enumerate() {
            labeled_cell(
                &mut rd,
                origin.x + i as f64 * col_w,
                origin.y,
                col_w,
                row_h,
                label,
                value,
            );
        }

        rd
    }
}

// ============================================================================
// Revision table
// ============================================================================

/// One row of the revision table.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RevisionRow {
    /// Revision letter (e.g. "A", "B").
    pub rev: String,
    /// Change description.
    pub description: String,
    /// Date string.
    pub date: String,
    /// Approver initials.
    pub approved_by: String,
}

/// A revision-history table entity. Rendered with the header row at the top
/// and revisions below, newest last (standard shop convention).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RevisionTable {
    /// Revision rows, oldest first.
    pub rows: Vec<RevisionRow>,
}

/// Column widths (mm) for the revision table: REV, DESCRIPTION, DATE, APPD.
const REV_COLS: [f64; 4] = [12.0, 78.0, 26.0, 14.0];
/// Row height (mm) for table rows.
const TABLE_ROW_H: f64 = 7.0;

/// Render a header + rows grid with the given column widths.
fn render_table(
    origin: Point2D,
    cols: &[f64],
    header: &[&str],
    rows: &[Vec<String>],
) -> RenderedDimension {
    let mut rd = RenderedDimension::new();
    let width: f64 = cols.iter().sum();
    let n_rows = rows.len() + 1;
    let height = n_rows as f64 * TABLE_ROW_H;
    let top = origin.y + height;

    // Horizontal lines.
    for i in 0..=n_rows {
        let y = top - i as f64 * TABLE_ROW_H;
        rd.add_line(Point2D::new(origin.x, y), Point2D::new(origin.x + width, y));
    }
    // Vertical lines.
    let mut x = origin.x;
    rd.add_line(Point2D::new(x, origin.y), Point2D::new(x, top));
    for w in cols {
        x += w;
        rd.add_line(Point2D::new(x, origin.y), Point2D::new(x, top));
    }

    let cell_text = |rd: &mut RenderedDimension, row: usize, col: usize, text: &str, h: f64| {
        let x0: f64 = origin.x + cols[..col].iter().sum::<f64>();
        let y = top - row as f64 * TABLE_ROW_H - TABLE_ROW_H / 2.0;
        rd.add_text(
            RenderedText::new(Point2D::new(x0 + cols[col] / 2.0, y), text, h)
                .with_alignment(TextAlignment::MiddleCenter),
        );
    };

    for (c, label) in header.iter().enumerate() {
        cell_text(&mut rd, 0, c, label, 2.2);
    }
    for (r, row) in rows.iter().enumerate() {
        for (c, value) in row.iter().enumerate() {
            cell_text(&mut rd, r + 1, c, value, 2.2);
        }
    }

    rd
}

impl RevisionTable {
    /// Create an empty revision table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a revision row.
    pub fn add_revision(
        &mut self,
        rev: impl Into<String>,
        description: impl Into<String>,
        date: impl Into<String>,
        approved_by: impl Into<String>,
    ) {
        self.rows.push(RevisionRow {
            rev: rev.into(),
            description: description.into(),
            date: date.into(),
            approved_by: approved_by.into(),
        });
    }

    /// Total rendered width in mm.
    pub fn width(&self) -> f64 {
        REV_COLS.iter().sum()
    }

    /// Total rendered height in mm (header + rows).
    pub fn height(&self) -> f64 {
        (self.rows.len() + 1) as f64 * TABLE_ROW_H
    }

    /// Render with the bottom-left corner at `origin`.
    pub fn render(&self, origin: Point2D) -> RenderedDimension {
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|r| {
                vec![
                    r.rev.clone(),
                    r.description.clone(),
                    r.date.clone(),
                    r.approved_by.clone(),
                ]
            })
            .collect();
        render_table(
            origin,
            &REV_COLS,
            &["REV", "DESCRIPTION", "DATE", "APPD"],
            &rows,
        )
    }
}

// ============================================================================
// BOM (bill of materials) table + balloons
// ============================================================================

/// One row of the bill of materials.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BomRow {
    /// Item (balloon) number, 1-based.
    pub item: u32,
    /// Part name.
    pub name: String,
    /// Quantity in the assembly.
    pub qty: u32,
    /// Material or spec column.
    pub material: String,
}

/// Column widths (mm) for the BOM table: ITEM, PART NAME, QTY, MATERIAL.
const BOM_COLS: [f64; 4] = [14.0, 72.0, 14.0, 40.0];

/// A bill-of-materials table entity for assembly drawings.
///
/// Build it from the document's part list with [`BomTable::from_parts`];
/// item numbers are assigned 1-based in list order and match the numbers
/// shown in [`BomBalloon`]s.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BomTable {
    /// BOM rows, item order.
    pub rows: Vec<BomRow>,
}

impl BomTable {
    /// Create an empty BOM table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number rows 1-based from a part list of `(name, qty, material)`.
    pub fn from_parts<N, M>(parts: impl IntoIterator<Item = (N, u32, M)>) -> Self
    where
        N: Into<String>,
        M: Into<String>,
    {
        let rows = parts
            .into_iter()
            .enumerate()
            .map(|(i, (name, qty, material))| BomRow {
                item: (i + 1) as u32,
                name: name.into(),
                qty,
                material: material.into(),
            })
            .collect();
        Self { rows }
    }

    /// Total rendered width in mm.
    pub fn width(&self) -> f64 {
        BOM_COLS.iter().sum()
    }

    /// Total rendered height in mm (header + rows).
    pub fn height(&self) -> f64 {
        (self.rows.len() + 1) as f64 * TABLE_ROW_H
    }

    /// Render with the bottom-left corner at `origin`.
    pub fn render(&self, origin: Point2D) -> RenderedDimension {
        let rows: Vec<Vec<String>> = self
            .rows
            .iter()
            .map(|r| {
                vec![
                    r.item.to_string(),
                    r.name.clone(),
                    r.qty.to_string(),
                    r.material.clone(),
                ]
            })
            .collect();
        render_table(
            origin,
            &BOM_COLS,
            &["ITEM", "PART NAME", "QTY", "MATERIAL"],
            &rows,
        )
    }
}

/// A BOM balloon: a numbered bubble with a leader pointing at a part
/// instance in an assembly view. The number references a [`BomTable`] row.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BomBalloon {
    /// Item number (matches a `BomRow::item`).
    pub item: u32,
    /// Leader tip: the point on the part being ballooned.
    pub anchor: Point2D,
    /// Center of the bubble.
    pub bubble: Point2D,
}

/// Bubble radius in mm.
const BALLOON_RADIUS: f64 = 4.0;

impl BomBalloon {
    /// Create a new balloon.
    pub fn new(item: u32, anchor: Point2D, bubble: Point2D) -> Self {
        Self {
            item,
            anchor,
            bubble,
        }
    }

    /// Render bubble + leader + arrow + item number.
    pub fn render(&self, style: &DimensionStyle) -> RenderedDimension {
        let mut rd = RenderedDimension::new();

        rd.add_arc(RenderedArc::new(
            self.bubble,
            BALLOON_RADIUS,
            0.0,
            std::f64::consts::TAU,
        ));

        // Leader from the bubble rim toward the anchor.
        let dx = self.anchor.x - self.bubble.x;
        let dy = self.anchor.y - self.bubble.y;
        let len = dx.hypot(dy);
        if len > BALLOON_RADIUS {
            let start = Point2D::new(
                self.bubble.x + dx / len * BALLOON_RADIUS,
                self.bubble.y + dy / len * BALLOON_RADIUS,
            );
            rd.add_line(start, self.anchor);
            rd.add_arrow(crate::dimension::RenderedArrow::new(
                self.anchor,
                dy.atan2(dx),
                ArrowType::ClosedFilled,
                style.arrow_size,
            ));
        }

        rd.add_text(
            RenderedText::new(self.bubble, self.item.to_string(), style.text_height * 1.2)
                .with_alignment(TextAlignment::MiddleCenter),
        );

        rd
    }
}

// ============================================================================
// Sheet model
// ============================================================================

/// Standard sheet sizes (landscape, mm).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SheetSize {
    /// ISO A4 landscape (297 × 210 mm).
    A4,
    /// ISO A3 landscape (420 × 297 mm).
    A3,
    /// ANSI A ("Letter") landscape (279.4 × 215.9 mm).
    Letter,
    /// Custom size in mm.
    Custom {
        /// Sheet width in mm.
        width: f64,
        /// Sheet height in mm.
        height: f64,
    },
}

impl SheetSize {
    /// Sheet dimensions `(width, height)` in mm.
    pub fn dimensions_mm(&self) -> (f64, f64) {
        match self {
            SheetSize::A4 => (297.0, 210.0),
            SheetSize::A3 => (420.0, 297.0),
            SheetSize::Letter => (279.4, 215.9),
            SheetSize::Custom { width, height } => (*width, *height),
        }
    }
}

/// A line on the sheet, in sheet mm coordinates (origin bottom-left).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetLine {
    /// Line classification (weight + dash).
    pub class: LineClass,
    /// Start point.
    pub start: Point2D,
    /// End point.
    pub end: Point2D,
}

/// An arc on the sheet.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetArc {
    /// Line classification.
    pub class: LineClass,
    /// The arc geometry.
    pub arc: RenderedArc,
}

/// A filled polygon on the sheet (arrowheads).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SheetPolygon {
    /// Line classification (fill uses the same color; weight ignored).
    pub class: LineClass,
    /// Polygon vertices.
    pub points: Vec<Point2D>,
}

/// A composed drawing sheet: a flat, classified display list in sheet mm
/// coordinates (origin at the bottom-left corner), ready for PDF or DXF
/// output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DrawingSheet {
    /// Sheet size.
    pub size: SheetSize,
    /// Border margin in mm.
    pub margin: f64,
    /// All line segments.
    pub lines: Vec<SheetLine>,
    /// All arcs.
    pub arcs: Vec<SheetArc>,
    /// All filled polygons (arrowheads).
    pub polygons: Vec<SheetPolygon>,
    /// All text labels (positions in sheet coordinates).
    pub texts: Vec<RenderedText>,
}

impl DrawingSheet {
    /// Create a new sheet with a border rectangle at `margin` from the edges.
    pub fn new(size: SheetSize) -> Self {
        let mut sheet = Self {
            size,
            margin: 10.0,
            lines: Vec::new(),
            arcs: Vec::new(),
            polygons: Vec::new(),
            texts: Vec::new(),
        };
        sheet.add_border();
        sheet
    }

    fn add_border(&mut self) {
        let (w, h) = self.size.dimensions_mm();
        let m = self.margin;
        let corners = [
            Point2D::new(m, m),
            Point2D::new(w - m, m),
            Point2D::new(w - m, h - m),
            Point2D::new(m, h - m),
        ];
        for i in 0..4 {
            self.lines.push(SheetLine {
                class: LineClass::Border,
                start: corners[i],
                end: corners[(i + 1) % 4],
            });
        }
    }

    fn push_line(&mut self, class: LineClass, start: Point2D, end: Point2D) {
        self.lines.push(SheetLine { class, start, end });
    }

    /// Place a projected view: edges are scaled about the view's bounds
    /// center and translated so that center lands at `center` on the sheet.
    /// Visible edges map to [`LineClass::Visible`], hidden to
    /// [`LineClass::Hidden`].
    pub fn add_projected_view(&mut self, view: &ProjectedView, center: Point2D, scale: f64) {
        let c = view.bounds.center();
        let map = |p: Point2D| -> Point2D {
            Point2D::new(
                center.x + (p.x - c.x) * scale,
                center.y + (p.y - c.y) * scale,
            )
        };
        for edge in &view.edges {
            let class = match edge.visibility {
                Visibility::Visible => LineClass::Visible,
                Visibility::Hidden => LineClass::Hidden,
            };
            self.push_line(class, map(edge.start), map(edge.end));
        }
    }

    /// Place a section view: cut curves on [`LineClass::Section`], hatch
    /// lines on [`LineClass::Hatch`].
    pub fn add_section_view(&mut self, view: &SectionView, center: Point2D, scale: f64) {
        let c = view.bounds.center();
        let map = |p: Point2D| -> Point2D {
            Point2D::new(
                center.x + (p.x - c.x) * scale,
                center.y + (p.y - c.y) * scale,
            )
        };
        for curve in &view.curves {
            let n = curve.points.len();
            if n < 2 {
                continue;
            }
            for i in 0..n - 1 {
                self.push_line(
                    LineClass::Section,
                    map(curve.points[i]),
                    map(curve.points[i + 1]),
                );
            }
            if curve.is_closed {
                self.push_line(
                    LineClass::Section,
                    map(curve.points[n - 1]),
                    map(curve.points[0]),
                );
            }
        }
        for (a, b) in &view.hatch_lines {
            self.push_line(LineClass::Hatch, map(*a), map(*b));
        }
    }

    /// Place a detail view (already magnified; centered about its own
    /// origin) at `center` on the sheet.
    pub fn add_detail_view(&mut self, view: &DetailView, center: Point2D) {
        let c = view.bounds.center();
        let map = |p: Point2D| -> Point2D {
            Point2D::new(center.x + (p.x - c.x), center.y + (p.y - c.y))
        };
        for edge in &view.edges {
            let class = match edge.visibility {
                Visibility::Visible => LineClass::Visible,
                Visibility::Hidden => LineClass::Hidden,
            };
            self.push_line(class, map(edge.start), map(edge.end));
        }
    }

    /// Add rendered annotation primitives, translated by `offset` and tagged
    /// with `class`. Arrows become filled triangles (or open V lines for
    /// [`ArrowType::Open`] / tick strokes for [`ArrowType::Tick`]).
    pub fn add_annotation(&mut self, rd: &RenderedDimension, class: LineClass, offset: Point2D) {
        let map = |p: Point2D| Point2D::new(p.x + offset.x, p.y + offset.y);

        for (a, b) in &rd.lines {
            self.push_line(class, map(*a), map(*b));
        }
        for arc in &rd.arcs {
            self.arcs.push(SheetArc {
                class,
                arc: RenderedArc::new(map(arc.center), arc.radius, arc.start_angle, arc.end_angle),
            });
        }
        for arrow in &rd.arrows {
            match arrow.arrow_type {
                ArrowType::Open => {
                    let ((a1, b1), (a2, b2)) = arrow.open_arrowhead_lines();
                    self.push_line(class, map(a1), map(b1));
                    self.push_line(class, map(a2), map(b2));
                }
                ArrowType::Tick => {
                    let half = arrow.size / 2.0;
                    let a = arrow.direction + std::f64::consts::FRAC_PI_4;
                    let p1 =
                        Point2D::new(arrow.tip.x - half * a.cos(), arrow.tip.y - half * a.sin());
                    let p2 =
                        Point2D::new(arrow.tip.x + half * a.cos(), arrow.tip.y + half * a.sin());
                    self.push_line(class, map(p1), map(p2));
                }
                ArrowType::ClosedFilled | ArrowType::ClosedBlank => {
                    let (tip, p1, p2) = arrow.arrowhead_points();
                    if arrow.arrow_type == ArrowType::ClosedFilled {
                        self.polygons.push(SheetPolygon {
                            class,
                            points: vec![map(tip), map(p1), map(p2)],
                        });
                    } else {
                        self.push_line(class, map(tip), map(p1));
                        self.push_line(class, map(p1), map(p2));
                        self.push_line(class, map(p2), map(tip));
                    }
                }
                ArrowType::Dot => {
                    self.arcs.push(SheetArc {
                        class,
                        arc: RenderedArc::new(
                            map(arrow.tip),
                            arrow.size / 2.0,
                            0.0,
                            std::f64::consts::TAU,
                        ),
                    });
                }
                ArrowType::None => {}
            }
        }
        for text in &rd.texts {
            let mut t = text.clone();
            t.position = map(t.position);
            self.texts.push(t);
        }
    }

    /// Place the title block in the bottom-right corner, inside the border.
    pub fn add_title_block(&mut self, block: &TitleBlock) {
        let (w, _) = self.size.dimensions_mm();
        let origin = Point2D::new(w - self.margin - block.width, self.margin);
        let rd = block.render(origin);
        self.add_annotation(&rd, LineClass::Border, Point2D::ORIGIN);
    }

    /// Place the revision table in the top-right corner, inside the border.
    pub fn add_revision_table(&mut self, table: &RevisionTable) {
        let (w, h) = self.size.dimensions_mm();
        let origin = Point2D::new(
            w - self.margin - table.width(),
            h - self.margin - table.height(),
        );
        let rd = table.render(origin);
        self.add_annotation(&rd, LineClass::Border, Point2D::ORIGIN);
    }

    /// Place the BOM table just above the title block on the right edge.
    ///
    /// `above_height` is the vertical clearance to leave under the table
    /// (typically the title block height).
    pub fn add_bom_table(&mut self, table: &BomTable, above_height: f64) {
        let (w, _) = self.size.dimensions_mm();
        let origin = Point2D::new(
            w - self.margin - table.width(),
            self.margin + above_height + 4.0,
        );
        let rd = table.render(origin);
        self.add_annotation(&rd, LineClass::Border, Point2D::ORIGIN);
    }

    /// Bounding box of everything currently on the sheet.
    pub fn content_bounds(&self) -> BoundingBox2D {
        let mut bb = BoundingBox2D::empty();
        for line in &self.lines {
            bb.include_point(line.start);
            bb.include_point(line.end);
        }
        for arc in &self.arcs {
            bb.include_point(Point2D::new(
                arc.arc.center.x - arc.arc.radius,
                arc.arc.center.y - arc.arc.radius,
            ));
            bb.include_point(Point2D::new(
                arc.arc.center.x + arc.arc.radius,
                arc.arc.center.y + arc.arc.radius,
            ));
        }
        for poly in &self.polygons {
            for p in &poly.points {
                bb.include_point(*p);
            }
        }
        for t in &self.texts {
            bb.include_point(t.position);
        }
        bb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_class_weights_ordered() {
        assert!(LineClass::Border.weight_mm() > LineClass::Visible.weight_mm());
        assert!(LineClass::Visible.weight_mm() > LineClass::Hidden.weight_mm());
        assert!(LineClass::Hidden.weight_mm() > LineClass::Dimension.weight_mm());
        assert!(LineClass::Hidden.dash_pattern_mm().len() == 2);
        assert!(LineClass::Visible.dash_pattern_mm().is_empty());
    }

    #[test]
    fn title_block_renders_all_fields() {
        let block = TitleBlock::new(TitleBlockFields {
            part_name: "BRACKET".into(),
            material: "6061-T6".into(),
            finish: "ANODIZE".into(),
            scale: "1:1".into(),
            drawn_by: "CP".into(),
            date: "2026-07-17".into(),
            revision: "B".into(),
            units: "MM".into(),
            tolerance_note: "±0.1".into(),
        });
        let rd = block.render(Point2D::ORIGIN);
        let all_text: Vec<&str> = rd.texts.iter().map(|t| t.text.as_str()).collect();
        for expected in [
            "BRACKET",
            "6061-T6",
            "ANODIZE",
            "1:1",
            "CP",
            "2026-07-17",
            "B",
            "MM",
        ] {
            assert!(all_text.contains(&expected), "missing field {expected}");
        }
        // 9 cells → 36 frame lines.
        assert_eq!(rd.lines.len(), 36);
    }

    #[test]
    fn revision_table_layout() {
        let mut table = RevisionTable::new();
        table.add_revision("A", "INITIAL RELEASE", "2026-07-01", "CP");
        table.add_revision("B", "HOLE DIA 5→6", "2026-07-17", "CP");
        assert!((table.height() - 21.0).abs() < 1e-9);
        let rd = table.render(Point2D::new(0.0, 0.0));
        assert!(rd.texts.iter().any(|t| t.text == "INITIAL RELEASE"));
        assert!(rd.texts.iter().any(|t| t.text == "APPD"));
        // 4 horizontal + 5 vertical lines.
        assert_eq!(rd.lines.len(), 9);
    }

    #[test]
    fn bom_from_parts_numbers_in_order() {
        let bom = BomTable::from_parts(vec![("BASE", 1u32, "6061"), ("PIN", 4u32, "303 SS")]);
        assert_eq!(bom.rows[0].item, 1);
        assert_eq!(bom.rows[1].item, 2);
        assert_eq!(bom.rows[1].qty, 4);
        let rd = bom.render(Point2D::ORIGIN);
        assert!(rd.texts.iter().any(|t| t.text == "PIN"));
    }

    #[test]
    fn balloon_renders_bubble_leader_arrow() {
        let balloon = BomBalloon::new(3, Point2D::new(0.0, 0.0), Point2D::new(20.0, 20.0));
        let rd = balloon.render(&DimensionStyle::default());
        assert_eq!(rd.arcs.len(), 1);
        assert_eq!(rd.arrows.len(), 1);
        assert!(rd.texts.iter().any(|t| t.text == "3"));
    }

    #[test]
    fn sheet_border_and_placement() {
        let mut sheet = DrawingSheet::new(SheetSize::A4);
        assert_eq!(sheet.lines.len(), 4); // border
        let block = TitleBlock::new(TitleBlockFields::default());
        sheet.add_title_block(&block);
        assert!(sheet.lines.len() > 4);
        let bb = sheet.content_bounds();
        let (w, h) = SheetSize::A4.dimensions_mm();
        assert!(bb.max_x <= w - sheet.margin + 1e-9);
        assert!(bb.max_y <= h - sheet.margin + 1e-9);
    }
}
