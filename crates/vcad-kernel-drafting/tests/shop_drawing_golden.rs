//! Golden-file test: a complete shop-ready drawing sheet rendered to PDF
//! must match the checked-in reference byte-for-byte.
//!
//! To regenerate after an intentional change:
//! `REGEN_GOLDEN=1 cargo test -p vcad-kernel-drafting --test shop_drawing_golden`

use vcad_kernel_drafting::{
    create_detail_view, detail_callout, detail_caption, project_mesh, sheet_to_pdf, BomBalloon,
    BomTable, DetailViewParams, DimensionStyle, DrawingSheet, HatchPattern, LineClass,
    LinearDimension, OffsetSectionPlane, OffsetSectionStep, Point2D, RevisionTable, SectionCutLine,
    SectionPlane, SheetSize, TitleBlock, TitleBlockFields, ViewDirection,
};
use vcad_kernel_tessellate::TriangleMesh;

/// A 60×40×20 mm block: enough geometry for views, a section, and a detail.
fn make_block() -> TriangleMesh {
    let (sx, sy, sz) = (60.0f32, 40.0f32, 20.0f32);
    #[rustfmt::skip]
    let vertices: Vec<f32> = vec![
        0.0, 0.0, 0.0,
        sx, 0.0, 0.0,
        sx, sy, 0.0,
        0.0, sy, 0.0,
        0.0, 0.0, sz,
        sx, 0.0, sz,
        sx, sy, sz,
        0.0, sy, sz,
    ];
    #[rustfmt::skip]
    let indices: Vec<u32> = vec![
        0, 2, 1, 0, 3, 2,
        4, 5, 6, 4, 6, 7,
        0, 1, 5, 0, 5, 4,
        2, 3, 7, 2, 7, 6,
        0, 4, 7, 0, 7, 3,
        1, 2, 6, 1, 6, 5,
    ];
    TriangleMesh {
        vertices,
        indices,
        normals: Vec::new(),
        face_kinds: Vec::new(),
    }
}

/// Compose the reference drawing: front view with a cutting-plane callout
/// and a dimension, an offset section, a detail view with bubble + caption,
/// BOM balloon + table, revision table, and title block.
fn build_sheet() -> DrawingSheet {
    let mesh = make_block();
    let style = DimensionStyle::default();
    let mut sheet = DrawingSheet::new(SheetSize::A3);

    // Front view, upper-left quadrant.
    let front = project_mesh(&mesh, ViewDirection::Front);
    let front_center = Point2D::new(110.0, 200.0);
    sheet.add_projected_view(&front, front_center, 1.0);

    // Cutting-plane callout across the front view (horizontal cut at mid-height).
    let cut = SectionCutLine::straight(
        Point2D::new(75.0, 200.0),
        Point2D::new(145.0, 200.0),
        "A",
        std::f64::consts::FRAC_PI_2,
    );
    sheet.add_annotation(
        &cut.render().unwrap(),
        LineClass::CuttingPlane,
        Point2D::ORIGIN,
    );

    // Width dimension under the front view.
    let dim =
        LinearDimension::horizontal(Point2D::new(80.0, 190.0), Point2D::new(140.0, 190.0), -12.0);
    if let Some(rd) = dim.render(None, &style) {
        sheet.add_annotation(&rd, LineClass::Dimension, Point2D::ORIGIN);
    }

    // Offset section A-A (jogged at the block midplane), upper-right quadrant.
    let section_plane = OffsetSectionPlane::new(
        SectionPlane::horizontal(6.0),
        vec![
            OffsetSectionStep::new(-1.0, 30.0, 0.0),
            OffsetSectionStep::new(30.0, 61.0, 8.0),
        ],
    );
    let section = vcad_kernel_drafting::offset_section_mesh(
        &mesh,
        &section_plane,
        Some(&HatchPattern::STANDARD_45),
    );
    let section_center = Point2D::new(300.0, 200.0);
    sheet.add_section_view(&section, section_center, 1.0);
    sheet.texts.push(vcad_kernel_drafting::RenderedText::new(
        Point2D::new(300.0, 165.0),
        "SECTION A-A",
        3.0,
    ));

    // Detail view of the front view's lower-left corner, lower-left quadrant.
    // Params are in the parent view's own coordinates (block corner ~ (0, 0));
    // the callout is drawn on the sheet by translating with the same
    // view-center → sheet-center offset used by add_projected_view.
    let detail_params = DetailViewParams::new(Point2D::new(4.0, 4.0), 2.0, 16.0, 12.0, "B");
    let view_to_sheet = Point2D::new(
        front_center.x - front.bounds.center().x,
        front_center.y - front.bounds.center().y,
    );
    sheet.add_annotation(
        &detail_callout(&detail_params, &style),
        LineClass::Dimension,
        view_to_sheet,
    );
    let detail = create_detail_view(&front, &detail_params);
    assert!(!detail.edges.is_empty(), "detail region must capture edges");
    let detail_center = Point2D::new(100.0, 90.0);
    sheet.add_detail_view(&detail, detail_center);
    let caption_pos = Point2D::new(detail_center.x, detail_center.y - 25.0);
    sheet.add_annotation(
        &detail_caption(&detail, caption_pos, &style),
        LineClass::Dimension,
        Point2D::ORIGIN,
    );

    // BOM balloon on the front view + table above the title block.
    let balloon = BomBalloon::new(1, Point2D::new(140.0, 210.0), Point2D::new(160.0, 230.0));
    sheet.add_annotation(
        &balloon.render(&style),
        LineClass::Dimension,
        Point2D::ORIGIN,
    );
    let bom = BomTable::from_parts(vec![("BLOCK", 1u32, "6061-T6 AL")]);
    let title = TitleBlock::new(TitleBlockFields {
        part_name: "TEST BLOCK".into(),
        material: "6061-T6 AL".into(),
        finish: "AS MACHINED".into(),
        scale: "1:1".into(),
        drawn_by: "VCAD".into(),
        date: "2026-07-17".into(),
        revision: "B".into(),
        units: "MM".into(),
        tolerance_note: "\u{b1}0.1".into(),
    });
    sheet.add_bom_table(&bom, title.height);
    sheet.add_title_block(&title);

    // Revision table, top-right.
    let mut revs = RevisionTable::new();
    revs.add_revision("A", "INITIAL RELEASE", "2026-07-01", "VC");
    revs.add_revision("B", "ADD SECTION A-A", "2026-07-17", "VC");
    sheet.add_revision_table(&revs);

    sheet
}

const GOLDEN_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/golden/shop_drawing.pdf");

#[test]
fn shop_drawing_matches_checked_in_golden_pdf() {
    let pdf = sheet_to_pdf(&build_sheet());

    if std::env::var("REGEN_GOLDEN").is_ok() {
        std::fs::write(GOLDEN_PATH, &pdf).unwrap();
        return;
    }

    let golden = std::fs::read(GOLDEN_PATH).expect(
        "missing golden file; run REGEN_GOLDEN=1 cargo test -p vcad-kernel-drafting \
         --test shop_drawing_golden to create it",
    );
    assert_eq!(
        pdf, golden,
        "rendered drawing differs from tests/golden/shop_drawing.pdf; if the \
         change is intentional, regenerate with REGEN_GOLDEN=1"
    );
}

#[test]
fn shop_drawing_pdf_is_deterministic() {
    assert_eq!(sheet_to_pdf(&build_sheet()), sheet_to_pdf(&build_sheet()));
}

#[test]
fn shop_drawing_has_all_line_classes() {
    let sheet = build_sheet();
    for class in [
        LineClass::Border,
        LineClass::Visible,
        LineClass::Section,
        LineClass::Hatch,
        LineClass::Dimension,
        LineClass::CuttingPlane,
    ] {
        assert!(
            sheet.lines.iter().any(|l| l.class == class),
            "sheet should contain {class:?} lines"
        );
    }
    assert!(sheet.texts.iter().any(|t| t.text == "SECTION A-A"));
    assert!(sheet.texts.iter().any(|t| t.text == "DETAIL B (SCALE 2:1)"));
    assert!(sheet.texts.iter().any(|t| t.text == "TEST BLOCK"));
}
