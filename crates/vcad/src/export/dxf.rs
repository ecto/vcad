//! DXF export for 2D laser cutting profiles.
//!
//! Exports flat profiles as DXF R12 format for SendCutSend and other
//! laser cutting services. Supports:
//! - Cut lines (layer "0" - default)
//! - Bend lines (layer "BEND" - for forming services)

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

/// A 2D point for DXF export.
#[derive(Debug, Clone, Copy)]
pub struct Point2D {
    /// X coordinate.
    pub x: f64,
    /// Y coordinate.
    pub y: f64,
}

impl Point2D {
    /// Create a new 2D point.
    pub fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// A 2D shape for DXF export.
#[derive(Debug, Clone)]
#[allow(missing_docs)]
pub enum Shape2D {
    /// Rectangular outline.
    Rectangle {
        /// Width in drawing units.
        width: f64,
        /// Height in drawing units.
        height: f64,
        /// Center position.
        center: Point2D,
    },
    /// Circle (for holes).
    Circle {
        /// Center position.
        center: Point2D,
        /// Circle radius.
        radius: f64,
    },
    /// Rounded rectangle (for slots with corner radii).
    RoundedRectangle {
        /// Overall width.
        width: f64,
        /// Overall height.
        height: f64,
        /// Center position.
        center: Point2D,
        /// Fillet radius for corners.
        corner_radius: f64,
    },
    /// Line segment (for bend lines, etc.).
    Line {
        /// Start point.
        start: Point2D,
        /// End point.
        end: Point2D,
        /// DXF layer name (e.g. `"0"` or `"BEND"`).
        layer: String,
    },
    /// Slot (stadium shape — rectangle with semicircular ends).
    Slot {
        /// Overall width.
        width: f64,
        /// Overall height.
        height: f64,
        /// Center position.
        center: Point2D,
    },
    /// Arbitrary closed polyline (for complex profiles).
    Polyline {
        /// Ordered list of vertices.
        points: Vec<Point2D>,
        /// Whether the polyline forms a closed loop.
        closed: bool,
    },
    /// Circular arc.
    Arc {
        /// Arc center.
        center: Point2D,
        /// Arc radius.
        radius: f64,
        /// Start angle in degrees.
        start_angle: f64,
        /// End angle in degrees.
        end_angle: f64,
    },
}

/// DXF document builder.
///
/// Accumulates 2D shapes and exports them as DXF R12 for laser cutting services.
pub struct DxfDocument {
    shapes: Vec<Shape2D>,
}

impl DxfDocument {
    /// Create a new empty DXF document.
    pub fn new() -> Self {
        Self { shapes: Vec::new() }
    }

    /// Add an arbitrary [`Shape2D`] to the document.
    pub fn add_shape(&mut self, shape: Shape2D) {
        self.shapes.push(shape);
    }

    /// Add a rectangular outline
    pub fn add_rectangle(&mut self, width: f64, height: f64, cx: f64, cy: f64) {
        self.shapes.push(Shape2D::Rectangle {
            width,
            height,
            center: Point2D::new(cx, cy),
        });
    }

    /// Add a circle (for holes)
    pub fn add_circle(&mut self, cx: f64, cy: f64, radius: f64) {
        self.shapes.push(Shape2D::Circle {
            center: Point2D::new(cx, cy),
            radius,
        });
    }

    /// Add a rounded rectangle (for slots)
    pub fn add_rounded_rectangle(
        &mut self,
        width: f64,
        height: f64,
        cx: f64,
        cy: f64,
        corner_radius: f64,
    ) {
        self.shapes.push(Shape2D::RoundedRectangle {
            width,
            height,
            center: Point2D::new(cx, cy),
            corner_radius,
        });
    }

    /// Add a line segment on the default layer
    pub fn add_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.shapes.push(Shape2D::Line {
            start: Point2D::new(x1, y1),
            end: Point2D::new(x2, y2),
            layer: "0".to_string(),
        });
    }

    /// Add a bend line (on BEND layer for forming services)
    pub fn add_bend_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.shapes.push(Shape2D::Line {
            start: Point2D::new(x1, y1),
            end: Point2D::new(x2, y2),
            layer: "BEND".to_string(),
        });
    }

    /// Add a slot (stadium shape - rectangle with semicircular ends)
    /// Used for louver vents and elongated cutouts
    pub fn add_slot(&mut self, width: f64, height: f64, cx: f64, cy: f64) {
        self.shapes.push(Shape2D::Slot {
            width,
            height,
            center: Point2D::new(cx, cy),
        });
    }

    /// Add an arbitrary closed polyline (for complex profiles)
    pub fn add_polyline(&mut self, points: Vec<(f64, f64)>, closed: bool) {
        self.shapes.push(Shape2D::Polyline {
            points: points
                .into_iter()
                .map(|(x, y)| Point2D::new(x, y))
                .collect(),
            closed,
        });
    }

    /// Add an arc (for rounded corners)
    pub fn add_arc(&mut self, cx: f64, cy: f64, radius: f64, start_angle: f64, end_angle: f64) {
        self.shapes.push(Shape2D::Arc {
            center: Point2D::new(cx, cy),
            radius,
            start_angle,
            end_angle,
        });
    }

    /// Generate knuckle hinge tab profile points along an edge
    /// Returns points for a profile with tabs extending in the +Y direction
    /// `edge_y` - Y coordinate of the edge
    /// `edge_x_start` - X coordinate of edge start
    /// `edge_x_end` - X coordinate of edge end
    /// `tab_width` - Width of each tab
    /// `tab_height` - Height of each tab (how far they extend)
    /// `num_tabs` - Number of tabs
    /// `offset` - If true, offset tabs (for mating piece)
    pub fn knuckle_tab_edge_points(
        edge_y: f64,
        edge_x_start: f64,
        edge_x_end: f64,
        _tab_width: f64,
        tab_height: f64,
        num_tabs: usize,
        offset: bool,
    ) -> Vec<(f64, f64)> {
        let mut points = Vec::new();
        let edge_length = edge_x_end - edge_x_start;
        let total_tabs = num_tabs * 2 - 1; // tabs + gaps
        let segment_width = edge_length / total_tabs as f64;

        // Start from left edge
        let mut x = edge_x_start;
        let start_with_tab = !offset;

        for i in 0..total_tabs {
            let is_tab = if start_with_tab {
                i % 2 == 0
            } else {
                i % 2 == 1
            };

            if is_tab {
                // Tab: go up, across, down
                points.push((x, edge_y));
                points.push((x, edge_y + tab_height));
                points.push((x + segment_width, edge_y + tab_height));
                points.push((x + segment_width, edge_y));
            } else {
                // Gap: just the edge (will connect to next segment)
                points.push((x, edge_y));
                points.push((x + segment_width, edge_y));
            }
            x += segment_width;
        }

        // Remove duplicate points where segments meet
        let mut deduped: Vec<(f64, f64)> = Vec::new();
        for p in points {
            if deduped.is_empty() || {
                let last = deduped.last().unwrap();
                (last.0 - p.0).abs() > 0.001 || (last.1 - p.1).abs() > 0.001
            } {
                deduped.push(p);
            }
        }

        deduped
    }

    /// Export to DXF file
    pub fn export(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // DXF Header
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HEADER")?;
        writeln!(writer, "9")?;
        writeln!(writer, "$ACADVER")?;
        writeln!(writer, "1")?;
        writeln!(writer, "AC1009")?; // DXF R12
        writeln!(writer, "9")?;
        writeln!(writer, "$INSUNITS")?;
        writeln!(writer, "70")?;
        writeln!(writer, "4")?; // Millimeters
        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        // Tables section (minimal)
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "TABLES")?;
        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        // Entities section
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "ENTITIES")?;

        for shape in &self.shapes {
            match shape {
                Shape2D::Rectangle {
                    width,
                    height,
                    center,
                } => {
                    self.write_rectangle(&mut writer, *width, *height, center)?;
                }
                Shape2D::Circle { center, radius } => {
                    self.write_circle(&mut writer, center, *radius)?;
                }
                Shape2D::RoundedRectangle {
                    width,
                    height,
                    center,
                    corner_radius,
                } => {
                    self.write_rounded_rectangle(
                        &mut writer,
                        *width,
                        *height,
                        center,
                        *corner_radius,
                    )?;
                }
                Shape2D::Line { start, end, layer } => {
                    self.write_line(&mut writer, start, end, layer)?;
                }
                Shape2D::Slot {
                    width,
                    height,
                    center,
                } => {
                    self.write_slot(&mut writer, *width, *height, center)?;
                }
                Shape2D::Polyline { points, closed } => {
                    self.write_polyline(&mut writer, points, *closed)?;
                }
                Shape2D::Arc {
                    center,
                    radius,
                    start_angle,
                    end_angle,
                } => {
                    self.write_arc(&mut writer, center, *radius, *start_angle, *end_angle)?;
                }
            }
        }

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        // End of file
        writeln!(writer, "0")?;
        writeln!(writer, "EOF")?;

        Ok(())
    }

    fn write_rectangle(
        &self,
        writer: &mut impl Write,
        width: f64,
        height: f64,
        center: &Point2D,
    ) -> std::io::Result<()> {
        let x1 = center.x - width / 2.0;
        let y1 = center.y - height / 2.0;
        let x2 = center.x + width / 2.0;
        let y2 = center.y + height / 2.0;

        // LWPOLYLINE (lightweight polyline)
        writeln!(writer, "0")?;
        writeln!(writer, "LWPOLYLINE")?;
        writeln!(writer, "8")?;
        writeln!(writer, "0")?; // Layer 0
        writeln!(writer, "90")?;
        writeln!(writer, "4")?; // 4 vertices
        writeln!(writer, "70")?;
        writeln!(writer, "1")?; // Closed polyline

        // Vertex 1 (bottom-left)
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1)?;

        // Vertex 2 (bottom-right)
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1)?;

        // Vertex 3 (top-right)
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2)?;

        // Vertex 4 (top-left)
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2)?;

        Ok(())
    }

    fn write_circle(
        &self,
        writer: &mut impl Write,
        center: &Point2D,
        radius: f64,
    ) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "CIRCLE")?;
        writeln!(writer, "8")?;
        writeln!(writer, "0")?; // Layer 0
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", center.x)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", center.y)?;
        writeln!(writer, "40")?;
        writeln!(writer, "{:.6}", radius)?;

        Ok(())
    }

    fn write_rounded_rectangle(
        &self,
        writer: &mut impl Write,
        width: f64,
        height: f64,
        center: &Point2D,
        corner_radius: f64,
    ) -> std::io::Result<()> {
        // For rounded rectangles, we approximate with a polyline
        // For simplicity, use arcs at corners

        let r = corner_radius.min(width / 2.0).min(height / 2.0);
        let x1 = center.x - width / 2.0;
        let y1 = center.y - height / 2.0;
        let x2 = center.x + width / 2.0;
        let y2 = center.y + height / 2.0;

        // Use a polyline with bulge values for rounded corners
        writeln!(writer, "0")?;
        writeln!(writer, "LWPOLYLINE")?;
        writeln!(writer, "8")?;
        writeln!(writer, "0")?;
        writeln!(writer, "90")?;
        writeln!(writer, "8")?; // 8 vertices (2 per corner)
        writeln!(writer, "70")?;
        writeln!(writer, "1")?; // Closed

        // Bulge value for 90-degree arc: tan(45°/2) = 0.414
        let bulge = 0.414213562; // tan(π/8)

        // Bottom edge
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1 + r)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1)?;

        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2 - r)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1)?;
        writeln!(writer, "42")?;
        writeln!(writer, "{:.6}", bulge)?; // Bulge for corner arc

        // Right edge
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1 + r)?;

        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2 - r)?;
        writeln!(writer, "42")?;
        writeln!(writer, "{:.6}", bulge)?;

        // Top edge
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x2 - r)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2)?;

        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1 + r)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2)?;
        writeln!(writer, "42")?;
        writeln!(writer, "{:.6}", bulge)?;

        // Left edge
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y2 - r)?;

        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", x1)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", y1 + r)?;
        writeln!(writer, "42")?;
        writeln!(writer, "{:.6}", bulge)?;

        Ok(())
    }

    fn write_line(
        &self,
        writer: &mut impl Write,
        start: &Point2D,
        end: &Point2D,
        layer: &str,
    ) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "LINE")?;
        writeln!(writer, "8")?;
        writeln!(writer, "{}", layer)?; // Layer name
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", start.x)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", start.y)?;
        writeln!(writer, "11")?;
        writeln!(writer, "{:.6}", end.x)?;
        writeln!(writer, "21")?;
        writeln!(writer, "{:.6}", end.y)?;

        Ok(())
    }

    fn write_slot(
        &self,
        writer: &mut impl Write,
        width: f64,
        height: f64,
        center: &Point2D,
    ) -> std::io::Result<()> {
        // Stadium shape: rectangle with semicircular ends
        // The semicircles are on the shorter dimension
        let (long, short) = if width >= height {
            (width, height)
        } else {
            (height, width)
        };

        let r = short / 2.0;
        let straight = long - short; // Length of straight portion

        if width >= height {
            // Horizontal slot
            let x1 = center.x - straight / 2.0;
            let x2 = center.x + straight / 2.0;

            // LWPOLYLINE with 4 vertices and bulges for semicircles
            writeln!(writer, "0")?;
            writeln!(writer, "LWPOLYLINE")?;
            writeln!(writer, "8")?;
            writeln!(writer, "0")?;
            writeln!(writer, "90")?;
            writeln!(writer, "4")?; // 4 vertices
            writeln!(writer, "70")?;
            writeln!(writer, "1")?; // Closed

            // Bottom-left (start of left semicircle)
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", x1)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", center.y - r)?;
            writeln!(writer, "42")?;
            writeln!(writer, "1.0")?; // Bulge for 180° arc (semicircle)

            // Top-left (end of left semicircle)
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", x1)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", center.y + r)?;

            // Top-right (start of right semicircle)
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", x2)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", center.y + r)?;
            writeln!(writer, "42")?;
            writeln!(writer, "1.0")?; // Bulge for 180° arc

            // Bottom-right (end of right semicircle)
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", x2)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", center.y - r)?;
        } else {
            // Vertical slot
            let y1 = center.y - straight / 2.0;
            let y2 = center.y + straight / 2.0;

            writeln!(writer, "0")?;
            writeln!(writer, "LWPOLYLINE")?;
            writeln!(writer, "8")?;
            writeln!(writer, "0")?;
            writeln!(writer, "90")?;
            writeln!(writer, "4")?;
            writeln!(writer, "70")?;
            writeln!(writer, "1")?;

            // Left-bottom
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", center.x - r)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", y1)?;
            writeln!(writer, "42")?;
            writeln!(writer, "1.0")?;

            // Right-bottom
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", center.x + r)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", y1)?;

            // Right-top
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", center.x + r)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", y2)?;
            writeln!(writer, "42")?;
            writeln!(writer, "1.0")?;

            // Left-top
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", center.x - r)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", y2)?;
        }

        Ok(())
    }

    fn write_polyline(
        &self,
        writer: &mut impl Write,
        points: &[Point2D],
        closed: bool,
    ) -> std::io::Result<()> {
        if points.is_empty() {
            return Ok(());
        }

        writeln!(writer, "0")?;
        writeln!(writer, "LWPOLYLINE")?;
        writeln!(writer, "8")?;
        writeln!(writer, "0")?; // Layer 0
        writeln!(writer, "90")?;
        writeln!(writer, "{}", points.len())?;
        writeln!(writer, "70")?;
        writeln!(writer, "{}", if closed { 1 } else { 0 })?;

        for point in points {
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", point.x)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", point.y)?;
        }

        Ok(())
    }

    fn write_arc(
        &self,
        writer: &mut impl Write,
        center: &Point2D,
        radius: f64,
        start_angle: f64,
        end_angle: f64,
    ) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "ARC")?;
        writeln!(writer, "8")?;
        writeln!(writer, "0")?; // Layer 0
        writeln!(writer, "10")?;
        writeln!(writer, "{:.6}", center.x)?;
        writeln!(writer, "20")?;
        writeln!(writer, "{:.6}", center.y)?;
        writeln!(writer, "40")?;
        writeln!(writer, "{:.6}", radius)?;
        writeln!(writer, "50")?;
        writeln!(writer, "{:.6}", start_angle)?;
        writeln!(writer, "51")?;
        writeln!(writer, "{:.6}", end_angle)?;

        Ok(())
    }
}

impl Default for DxfDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// DXF document builder for technical drawings with visible/hidden line support.
///
/// Exports projected views with proper layer and linetype definitions:
/// - VISIBLE layer: continuous lines for visible edges
/// - HIDDEN layer: dashed lines for hidden edges
pub struct DxfDraftingDocument {
    lines: Vec<DraftingLine>,
}

/// A line in a drafting document with visibility information.
struct DraftingLine {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
    visible: bool,
}

impl DxfDraftingDocument {
    /// Create a new empty drafting document.
    pub fn new() -> Self {
        Self { lines: Vec::new() }
    }

    /// Add a visible line (continuous).
    pub fn add_visible_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.lines.push(DraftingLine {
            x1,
            y1,
            x2,
            y2,
            visible: true,
        });
    }

    /// Add a hidden line (dashed).
    pub fn add_hidden_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.lines.push(DraftingLine {
            x1,
            y1,
            x2,
            y2,
            visible: false,
        });
    }

    /// Export to DXF file with proper layer and linetype tables.
    pub fn export(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let file = File::create(path)?;
        let writer = BufWriter::new(file);
        self.export_to_writer(writer)
    }

    /// Export to a writer with proper layer and linetype tables.
    pub fn export_to_writer(&self, mut writer: impl Write) -> std::io::Result<()> {
        // DXF Header
        self.write_header(&mut writer)?;

        // Tables section with layers and linetypes
        self.write_tables(&mut writer)?;

        // Entities section
        self.write_entities(&mut writer)?;

        // End of file
        writeln!(writer, "0")?;
        writeln!(writer, "EOF")?;

        Ok(())
    }

    fn write_header(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HEADER")?;

        // AutoCAD version
        writeln!(writer, "9")?;
        writeln!(writer, "$ACADVER")?;
        writeln!(writer, "1")?;
        writeln!(writer, "AC1009")?; // DXF R12

        // Units = millimeters
        writeln!(writer, "9")?;
        writeln!(writer, "$INSUNITS")?;
        writeln!(writer, "70")?;
        writeln!(writer, "4")?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }

    fn write_tables(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "TABLES")?;

        // Linetype table
        self.write_ltype_table(writer)?;

        // Layer table
        self.write_layer_table(writer)?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }

    fn write_ltype_table(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "TABLE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "LTYPE")?;
        writeln!(writer, "70")?;
        writeln!(writer, "2")?; // 2 entries

        // CONTINUOUS linetype
        writeln!(writer, "0")?;
        writeln!(writer, "LTYPE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "CONTINUOUS")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "3")?;
        writeln!(writer, "Solid line")?;
        writeln!(writer, "72")?;
        writeln!(writer, "65")?;
        writeln!(writer, "73")?;
        writeln!(writer, "0")?;
        writeln!(writer, "40")?;
        writeln!(writer, "0.0")?;

        // HIDDEN linetype (dashed)
        writeln!(writer, "0")?;
        writeln!(writer, "LTYPE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HIDDEN")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "3")?;
        writeln!(writer, "Hidden line")?;
        writeln!(writer, "72")?;
        writeln!(writer, "65")?;
        writeln!(writer, "73")?;
        writeln!(writer, "2")?; // 2 dash elements
        writeln!(writer, "40")?;
        writeln!(writer, "9.525")?; // Total pattern length
        writeln!(writer, "49")?;
        writeln!(writer, "6.35")?; // Dash length
        writeln!(writer, "49")?;
        writeln!(writer, "-3.175")?; // Gap length (negative = space)

        writeln!(writer, "0")?;
        writeln!(writer, "ENDTAB")?;

        Ok(())
    }

    fn write_layer_table(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "TABLE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "70")?;
        writeln!(writer, "2")?; // 2 layers

        // VISIBLE layer - continuous, color 7 (white/black)
        writeln!(writer, "0")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "2")?;
        writeln!(writer, "VISIBLE")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "62")?;
        writeln!(writer, "7")?; // Color 7 (white/black)
        writeln!(writer, "6")?;
        writeln!(writer, "CONTINUOUS")?;

        // HIDDEN layer - hidden linetype, color 8 (gray)
        writeln!(writer, "0")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HIDDEN")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "62")?;
        writeln!(writer, "8")?; // Color 8 (gray)
        writeln!(writer, "6")?;
        writeln!(writer, "HIDDEN")?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDTAB")?;

        Ok(())
    }

    fn write_entities(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "ENTITIES")?;

        for line in &self.lines {
            writeln!(writer, "0")?;
            writeln!(writer, "LINE")?;
            writeln!(writer, "8")?;
            writeln!(
                writer,
                "{}",
                if line.visible { "VISIBLE" } else { "HIDDEN" }
            )?;
            writeln!(writer, "6")?;
            writeln!(
                writer,
                "{}",
                if line.visible { "CONTINUOUS" } else { "HIDDEN" }
            )?;
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", line.x1)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", line.y1)?;
            writeln!(writer, "11")?;
            writeln!(writer, "{:.6}", line.x2)?;
            writeln!(writer, "21")?;
            writeln!(writer, "{:.6}", line.y2)?;
        }

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }

    /// Number of visible lines.
    pub fn num_visible(&self) -> usize {
        self.lines.iter().filter(|l| l.visible).count()
    }

    /// Number of hidden lines.
    pub fn num_hidden(&self) -> usize {
        self.lines.iter().filter(|l| !l.visible).count()
    }
}

impl Default for DxfDraftingDocument {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// Section View DXF Export
// ============================================================================

/// DXF document builder for section views.
///
/// Exports section views with proper layer definitions:
/// - SECTION layer: solid lines for section cut curves
/// - HATCH layer: thinner lines for cross-hatching
pub struct DxfSectionDocument {
    section_lines: Vec<SectionLine>,
    hatch_lines: Vec<HatchLine>,
}

/// A section curve line.
struct SectionLine {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

/// A hatch line.
struct HatchLine {
    x1: f64,
    y1: f64,
    x2: f64,
    y2: f64,
}

impl DxfSectionDocument {
    /// Create a new empty section document.
    pub fn new() -> Self {
        Self {
            section_lines: Vec::new(),
            hatch_lines: Vec::new(),
        }
    }

    /// Add a section curve line.
    pub fn add_section_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.section_lines.push(SectionLine { x1, y1, x2, y2 });
    }

    /// Add a hatch line.
    pub fn add_hatch_line(&mut self, x1: f64, y1: f64, x2: f64, y2: f64) {
        self.hatch_lines.push(HatchLine { x1, y1, x2, y2 });
    }

    /// Add section curves from a polyline.
    pub fn add_section_polyline(&mut self, points: &[(f64, f64)], closed: bool) {
        if points.len() < 2 {
            return;
        }

        for i in 0..points.len() - 1 {
            let (x1, y1) = points[i];
            let (x2, y2) = points[i + 1];
            self.add_section_line(x1, y1, x2, y2);
        }

        if closed && points.len() >= 3 {
            let (x1, y1) = points[points.len() - 1];
            let (x2, y2) = points[0];
            self.add_section_line(x1, y1, x2, y2);
        }
    }

    /// Number of section lines.
    pub fn num_section_lines(&self) -> usize {
        self.section_lines.len()
    }

    /// Number of hatch lines.
    pub fn num_hatch_lines(&self) -> usize {
        self.hatch_lines.len()
    }

    /// Export to DXF file.
    pub fn export(&self, path: impl AsRef<Path>) -> std::io::Result<()> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);

        // DXF Header
        self.write_header(&mut writer)?;

        // Tables section with layers and linetypes
        self.write_tables(&mut writer)?;

        // Entities section
        self.write_entities(&mut writer)?;

        // End of file
        writeln!(writer, "0")?;
        writeln!(writer, "EOF")?;

        Ok(())
    }

    fn write_header(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HEADER")?;

        // AutoCAD version
        writeln!(writer, "9")?;
        writeln!(writer, "$ACADVER")?;
        writeln!(writer, "1")?;
        writeln!(writer, "AC1009")?; // DXF R12

        // Units = millimeters
        writeln!(writer, "9")?;
        writeln!(writer, "$INSUNITS")?;
        writeln!(writer, "70")?;
        writeln!(writer, "4")?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }

    fn write_tables(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "TABLES")?;

        // Linetype table
        self.write_ltype_table(writer)?;

        // Layer table
        self.write_layer_table(writer)?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }

    fn write_ltype_table(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "TABLE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "LTYPE")?;
        writeln!(writer, "70")?;
        writeln!(writer, "1")?; // 1 entry

        // CONTINUOUS linetype
        writeln!(writer, "0")?;
        writeln!(writer, "LTYPE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "CONTINUOUS")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "3")?;
        writeln!(writer, "Solid line")?;
        writeln!(writer, "72")?;
        writeln!(writer, "65")?;
        writeln!(writer, "73")?;
        writeln!(writer, "0")?;
        writeln!(writer, "40")?;
        writeln!(writer, "0.0")?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDTAB")?;

        Ok(())
    }

    fn write_layer_table(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "TABLE")?;
        writeln!(writer, "2")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "70")?;
        writeln!(writer, "2")?; // 2 layers

        // SECTION layer - thick, color 7 (white/black)
        writeln!(writer, "0")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "2")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "62")?;
        writeln!(writer, "7")?; // Color 7 (white/black)
        writeln!(writer, "6")?;
        writeln!(writer, "CONTINUOUS")?;

        // HATCH layer - thin, color 8 (gray)
        writeln!(writer, "0")?;
        writeln!(writer, "LAYER")?;
        writeln!(writer, "2")?;
        writeln!(writer, "HATCH")?;
        writeln!(writer, "70")?;
        writeln!(writer, "0")?;
        writeln!(writer, "62")?;
        writeln!(writer, "8")?; // Color 8 (gray)
        writeln!(writer, "6")?;
        writeln!(writer, "CONTINUOUS")?;

        writeln!(writer, "0")?;
        writeln!(writer, "ENDTAB")?;

        Ok(())
    }

    fn write_entities(&self, writer: &mut impl Write) -> std::io::Result<()> {
        writeln!(writer, "0")?;
        writeln!(writer, "SECTION")?;
        writeln!(writer, "2")?;
        writeln!(writer, "ENTITIES")?;

        // Section lines
        for line in &self.section_lines {
            writeln!(writer, "0")?;
            writeln!(writer, "LINE")?;
            writeln!(writer, "8")?;
            writeln!(writer, "SECTION")?;
            writeln!(writer, "6")?;
            writeln!(writer, "CONTINUOUS")?;
            // Line weight (thicker for section lines)
            writeln!(writer, "370")?;
            writeln!(writer, "50")?; // 0.50mm
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", line.x1)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", line.y1)?;
            writeln!(writer, "11")?;
            writeln!(writer, "{:.6}", line.x2)?;
            writeln!(writer, "21")?;
            writeln!(writer, "{:.6}", line.y2)?;
        }

        // Hatch lines
        for line in &self.hatch_lines {
            writeln!(writer, "0")?;
            writeln!(writer, "LINE")?;
            writeln!(writer, "8")?;
            writeln!(writer, "HATCH")?;
            writeln!(writer, "6")?;
            writeln!(writer, "CONTINUOUS")?;
            // Line weight (thinner for hatch lines)
            writeln!(writer, "370")?;
            writeln!(writer, "13")?; // 0.13mm
            writeln!(writer, "10")?;
            writeln!(writer, "{:.6}", line.x1)?;
            writeln!(writer, "20")?;
            writeln!(writer, "{:.6}", line.y1)?;
            writeln!(writer, "11")?;
            writeln!(writer, "{:.6}", line.x2)?;
            writeln!(writer, "21")?;
            writeln!(writer, "{:.6}", line.y2)?;
        }

        writeln!(writer, "0")?;
        writeln!(writer, "ENDSEC")?;

        Ok(())
    }
}

impl Default for DxfSectionDocument {
    fn default() -> Self {
        Self::new()
    }
}

/// Export a section view to a DXF file.
///
/// Creates a DXF with SECTION layer for cut curves and HATCH layer for hatching.
#[cfg(feature = "drafting")]
pub fn export_section_to_dxf(
    view: &vcad_kernel_drafting::SectionView,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    let mut doc = DxfSectionDocument::new();

    // Add section curves
    for curve in &view.curves {
        let points: Vec<(f64, f64)> = curve.points.iter().map(|p| (p.x, p.y)).collect();
        doc.add_section_polyline(&points, curve.is_closed);
    }

    // Add hatch lines
    for (p0, p1) in &view.hatch_lines {
        doc.add_hatch_line(p0.x, p0.y, p1.x, p1.y);
    }

    doc.export(path)
}

/// Export a projected view to a DXF drafting document.
///
/// This function takes a ProjectedView from the drafting crate and
/// creates a DxfDraftingDocument with proper visible/hidden line layers.
#[cfg(feature = "drafting")]
pub fn export_projected_view_to_dxf(
    view: &vcad_kernel_drafting::ProjectedView,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    use vcad_kernel_drafting::Visibility;

    let mut doc = DxfDraftingDocument::new();

    for edge in &view.edges {
        let (x1, y1) = (edge.start.x, edge.start.y);
        let (x2, y2) = (edge.end.x, edge.end.y);

        match edge.visibility {
            Visibility::Visible => doc.add_visible_line(x1, y1, x2, y2),
            Visibility::Hidden => doc.add_hidden_line(x1, y1, x2, y2),
        }
    }

    doc.export(path)
}

/// Export a projected view to a DXF byte buffer.
///
/// This function takes a ProjectedView from the drafting crate and
/// returns the DXF content as bytes for use in WASM or other contexts.
#[cfg(feature = "drafting")]
pub fn export_projected_view_to_dxf_buffer(
    view: &vcad_kernel_drafting::ProjectedView,
) -> std::io::Result<Vec<u8>> {
    use vcad_kernel_drafting::Visibility;

    let mut doc = DxfDraftingDocument::new();

    for edge in &view.edges {
        let (x1, y1) = (edge.start.x, edge.start.y);
        let (x2, y2) = (edge.end.x, edge.end.y);

        match edge.visibility {
            Visibility::Visible => doc.add_visible_line(x1, y1, x2, y2),
            Visibility::Hidden => doc.add_hidden_line(x1, y1, x2, y2),
        }
    }

    let mut buffer = Vec::new();
    doc.export_to_writer(&mut buffer)?;
    Ok(buffer)
}

/// Export a composed drawing sheet ([`vcad_kernel_drafting::DrawingSheet`])
/// to a DXF byte buffer.
///
/// Every [`vcad_kernel_drafting::LineClass`] maps to its own DXF layer
/// (VISIBLE, HIDDEN, SECTION, HATCH, DIMENSION, CUTPLANE, CENTER, BORDER)
/// with the matching linetype, so the title block, revision table, BOM
/// balloons/tables, cutting-plane callouts, and dimensions all round-trip
/// into CAD packages. Filled arrowheads become SOLID entities; text becomes
/// TEXT entities.
#[cfg(feature = "drafting")]
pub fn export_drawing_sheet_to_dxf_buffer(
    sheet: &vcad_kernel_drafting::DrawingSheet,
) -> std::io::Result<Vec<u8>> {
    use vcad_kernel_drafting::LineClass;

    let mut w: Vec<u8> = Vec::new();

    // Header.
    writeln!(w, "0\nSECTION\n2\nHEADER")?;
    writeln!(w, "9\n$ACADVER\n1\nAC1009")?;
    writeln!(w, "9\n$INSUNITS\n70\n4")?;
    writeln!(w, "0\nENDSEC")?;

    // Tables: linetypes then layers.
    writeln!(w, "0\nSECTION\n2\nTABLES")?;
    writeln!(w, "0\nTABLE\n2\nLTYPE\n70\n4")?;
    // (name, description, elements)
    let ltypes: [(&str, &str, &[f64]); 4] = [
        ("CONTINUOUS", "Solid line", &[]),
        ("HIDDEN", "Hidden line", &[6.35, -3.175]),
        (
            "PHANTOM",
            "Phantom line",
            &[12.7, -3.175, 3.175, -3.175, 3.175, -3.175],
        ),
        ("CENTER", "Center line", &[12.7, -3.175, 3.175, -3.175]),
    ];
    for (name, desc, elems) in ltypes {
        writeln!(w, "0\nLTYPE\n2\n{name}\n70\n0\n3\n{desc}\n72\n65")?;
        writeln!(w, "73\n{}", elems.len())?;
        let total: f64 = elems.iter().map(|e| e.abs()).sum();
        writeln!(w, "40\n{total:.4}")?;
        for e in elems {
            writeln!(w, "49\n{e:.4}")?;
        }
    }
    writeln!(w, "0\nENDTAB")?;

    let classes = [
        LineClass::Visible,
        LineClass::Hidden,
        LineClass::Section,
        LineClass::Hatch,
        LineClass::Dimension,
        LineClass::CuttingPlane,
        LineClass::Center,
        LineClass::Border,
    ];
    let ltype_of = |class: LineClass| -> &'static str {
        match class {
            LineClass::Hidden => "HIDDEN",
            LineClass::CuttingPlane => "PHANTOM",
            LineClass::Center => "CENTER",
            _ => "CONTINUOUS",
        }
    };
    let color_of = |class: LineClass| -> u8 {
        match class {
            LineClass::Visible | LineClass::Section | LineClass::Border => 7,
            LineClass::Hidden => 8,
            LineClass::Hatch => 8,
            LineClass::Dimension => 1,
            LineClass::CuttingPlane | LineClass::Center => 4,
        }
    };
    writeln!(w, "0\nTABLE\n2\nLAYER\n70\n{}", classes.len())?;
    for class in classes {
        writeln!(
            w,
            "0\nLAYER\n2\n{}\n70\n0\n62\n{}\n6\n{}",
            class.dxf_layer(),
            color_of(class),
            ltype_of(class)
        )?;
    }
    writeln!(w, "0\nENDTAB")?;
    writeln!(w, "0\nENDSEC")?;

    // Entities.
    writeln!(w, "0\nSECTION\n2\nENTITIES")?;
    for line in &sheet.lines {
        writeln!(w, "0\nLINE\n8\n{}", line.class.dxf_layer())?;
        writeln!(w, "6\n{}", ltype_of(line.class))?;
        writeln!(w, "10\n{:.6}\n20\n{:.6}", line.start.x, line.start.y)?;
        writeln!(w, "11\n{:.6}\n21\n{:.6}", line.end.x, line.end.y)?;
    }
    for arc in &sheet.arcs {
        let full_circle =
            (arc.arc.end_angle - arc.arc.start_angle).abs() >= std::f64::consts::TAU - 1e-9;
        if full_circle {
            writeln!(w, "0\nCIRCLE\n8\n{}", arc.class.dxf_layer())?;
            writeln!(
                w,
                "10\n{:.6}\n20\n{:.6}",
                arc.arc.center.x, arc.arc.center.y
            )?;
            writeln!(w, "40\n{:.6}", arc.arc.radius)?;
        } else {
            writeln!(w, "0\nARC\n8\n{}", arc.class.dxf_layer())?;
            writeln!(
                w,
                "10\n{:.6}\n20\n{:.6}",
                arc.arc.center.x, arc.arc.center.y
            )?;
            writeln!(w, "40\n{:.6}", arc.arc.radius)?;
            writeln!(w, "50\n{:.6}", arc.arc.start_angle.to_degrees())?;
            writeln!(w, "51\n{:.6}", arc.arc.end_angle.to_degrees())?;
        }
    }
    for poly in &sheet.polygons {
        // Filled arrowheads: DXF SOLID takes 3 or 4 corners (13/23 repeats
        // the third for triangles).
        if poly.points.len() >= 3 {
            writeln!(w, "0\nSOLID\n8\n{}", poly.class.dxf_layer())?;
            let p = &poly.points;
            writeln!(w, "10\n{:.6}\n20\n{:.6}", p[0].x, p[0].y)?;
            writeln!(w, "11\n{:.6}\n21\n{:.6}", p[1].x, p[1].y)?;
            writeln!(w, "12\n{:.6}\n22\n{:.6}", p[2].x, p[2].y)?;
            let last = if p.len() > 3 { &p[3] } else { &p[2] };
            writeln!(w, "13\n{:.6}\n23\n{:.6}", last.x, last.y)?;
        }
    }
    for text in &sheet.texts {
        writeln!(w, "0\nTEXT\n8\nDIMENSION")?;
        writeln!(w, "10\n{:.6}\n20\n{:.6}", text.position.x, text.position.y)?;
        writeln!(w, "40\n{:.6}", text.height)?;
        writeln!(w, "1\n{}", text.text)?;
        writeln!(w, "50\n{:.6}", text.rotation.to_degrees())?;
        writeln!(w, "72\n{}", text.alignment.dxf_horizontal())?;
        writeln!(w, "73\n{}", text.alignment.dxf_vertical())?;
        // Alignment point (required when 72/73 nonzero).
        writeln!(w, "11\n{:.6}\n21\n{:.6}", text.position.x, text.position.y)?;
    }
    writeln!(w, "0\nENDSEC")?;
    writeln!(w, "0\nEOF")?;

    Ok(w)
}

/// Export a composed drawing sheet to a DXF file. See
/// [`export_drawing_sheet_to_dxf_buffer`].
#[cfg(feature = "drafting")]
pub fn export_drawing_sheet_to_dxf(
    sheet: &vcad_kernel_drafting::DrawingSheet,
    path: impl AsRef<Path>,
) -> std::io::Result<()> {
    let buffer = export_drawing_sheet_to_dxf_buffer(sheet)?;
    std::fs::write(path, buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[cfg(feature = "drafting")]
    #[test]
    fn test_export_drawing_sheet_dxf() {
        use vcad_kernel_drafting::{
            BomBalloon, DimensionStyle, DrawingSheet, LineClass, Point2D, SectionCutLine,
            SheetSize, TitleBlock, TitleBlockFields,
        };

        let mut sheet = DrawingSheet::new(SheetSize::A4);
        sheet.add_title_block(&TitleBlock::new(TitleBlockFields {
            part_name: "DXF TEST".into(),
            ..Default::default()
        }));
        let cut = SectionCutLine::straight(
            Point2D::new(50.0, 100.0),
            Point2D::new(150.0, 100.0),
            "A",
            std::f64::consts::FRAC_PI_2,
        );
        sheet.add_annotation(
            &cut.render().unwrap(),
            LineClass::CuttingPlane,
            Point2D::ORIGIN,
        );
        let balloon = BomBalloon::new(2, Point2D::new(60.0, 120.0), Point2D::new(80.0, 140.0));
        sheet.add_annotation(
            &balloon.render(&DimensionStyle::default()),
            LineClass::Dimension,
            Point2D::ORIGIN,
        );

        let dxf = export_drawing_sheet_to_dxf_buffer(&sheet).unwrap();
        let text = String::from_utf8(dxf).unwrap();

        // Layers and linetypes present.
        for expected in ["BORDER", "CUTPLANE", "DIMENSION", "PHANTOM", "HIDDEN"] {
            assert!(text.contains(expected), "missing {expected}");
        }
        // Entities: lines, circle (balloon), solid (arrows), text fields.
        assert!(text.contains("\nLINE\n"));
        assert!(text.contains("\nCIRCLE\n"));
        assert!(text.contains("\nSOLID\n"));
        assert!(text.contains("\nDXF TEST\n"));
        assert!(text.trim_end().ends_with("EOF"));
    }

    #[test]
    fn test_dxf_section_document() {
        let mut doc = DxfSectionDocument::new();

        // Add section curves (a square)
        doc.add_section_polyline(&[(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)], true);

        // Add some hatch lines
        doc.add_hatch_line(0.0, 2.0, 10.0, 2.0);
        doc.add_hatch_line(0.0, 4.0, 10.0, 4.0);
        doc.add_hatch_line(0.0, 6.0, 10.0, 6.0);
        doc.add_hatch_line(0.0, 8.0, 10.0, 8.0);

        assert_eq!(doc.num_section_lines(), 4); // 4 sides of square
        assert_eq!(doc.num_hatch_lines(), 4);

        let path = "/tmp/test_section.dxf";
        doc.export(path).unwrap();

        let content = fs::read_to_string(path).unwrap();

        // Check structure
        assert!(content.contains("SECTION"));
        assert!(content.contains("HATCH"));
        assert!(content.contains("TABLES"));
        assert!(content.contains("LAYER"));
        assert!(content.contains("EOF"));
    }

    #[test]
    fn test_dxf_rectangle() {
        let mut doc = DxfDocument::new();
        doc.add_rectangle(100.0, 50.0, 0.0, 0.0);

        let path = "/tmp/test_rect.dxf";
        doc.export(path).unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("LWPOLYLINE"));
        assert!(content.contains("EOF"));
    }

    #[test]
    fn test_dxf_circle() {
        let mut doc = DxfDocument::new();
        doc.add_circle(10.0, 20.0, 5.0);

        let path = "/tmp/test_circle.dxf";
        doc.export(path).unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("CIRCLE"));
    }

    #[test]
    fn test_dxf_rounded_rectangle() {
        let mut doc = DxfDocument::new();
        doc.add_rounded_rectangle(100.0, 50.0, 0.0, 0.0, 10.0);

        let path = "/tmp/test_rounded.dxf";
        doc.export(path).unwrap();

        let content = fs::read_to_string(path).unwrap();
        assert!(content.contains("LWPOLYLINE"));
        assert!(content.contains("42")); // Bulge code
    }

    #[test]
    fn test_dxf_drafting_document() {
        let mut doc = DxfDraftingDocument::new();

        // Add some visible lines (front face of a square)
        doc.add_visible_line(0.0, 0.0, 10.0, 0.0);
        doc.add_visible_line(10.0, 0.0, 10.0, 10.0);
        doc.add_visible_line(10.0, 10.0, 0.0, 10.0);
        doc.add_visible_line(0.0, 10.0, 0.0, 0.0);

        // Add some hidden lines (back face)
        doc.add_hidden_line(2.0, 2.0, 12.0, 2.0);
        doc.add_hidden_line(12.0, 2.0, 12.0, 12.0);

        assert_eq!(doc.num_visible(), 4);
        assert_eq!(doc.num_hidden(), 2);

        let path = "/tmp/test_drafting.dxf";
        doc.export(path).unwrap();

        let content = fs::read_to_string(path).unwrap();

        // Check structure
        assert!(content.contains("SECTION"));
        assert!(content.contains("TABLES"));
        assert!(content.contains("LTYPE"));
        assert!(content.contains("LAYER"));
        assert!(content.contains("ENTITIES"));
        assert!(content.contains("EOF"));

        // Check linetypes
        assert!(content.contains("CONTINUOUS"));
        assert!(content.contains("HIDDEN"));

        // Check layers
        assert!(content.contains("VISIBLE"));
    }
}
