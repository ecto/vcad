# Drafting: shop-ready drawings (M0–M2)

Goal: close the gap between `vcad-kernel-drafting`'s projection/dimension/GD&T
primitives and a drawing a machine shop will quote from without calling.
All three milestones are **complete**; everything renders down to the crate's
existing `RenderedDimension` primitive vocabulary (lines, arcs, arrows,
texts), so the SVG (app), DXF, and PDF consumers share one seam.

## M0 — Section & detail views ✅

- **Full sections** already existed (`section_mesh`: plane–mesh intersection,
  polyline chaining, hole-aware 45° hatching).
- **Offset (stepped) sections** — `OffsetSectionPlane` (`types.rs`): a base
  `SectionPlane` plus `OffsetSectionStep { u_start, u_end, offset }` jogs.
  `offset_section_mesh` (`section.rs`) sections each parallel plane, projects
  into the *base* frame (jogs flatten, per drafting convention), clips closed
  regions to the step's U strip (Sutherland–Hodgman) and open curves
  parametrically, then hatches the clipped regions.
- **Cutting-plane callout on the parent view** — `SectionCutLine`
  (`section.rs`): trace polyline (jogs included for offset sections),
  viewing-direction arrows, letter labels at both ends, and
  `section_name()` → "SECTION A-A". Consumers draw the trace with
  `LineClass::CuttingPlane` (thick dash-dot / PHANTOM).
- **Detail bubbles with scale callout** — `detail_callout` draws the circle +
  leader + letter on the parent view (circumscribed radius of the capture
  rect); `DetailView::caption()` / `detail_caption` produce
  "DETAIL B (SCALE 2:1)" with `format_scale` (2.0 → "2:1", 0.5 → "1:2").

## M1 — Title block, revision table, BOM ✅

All in the new `sheet.rs`, as first-class entities that render to
`RenderedDimension` at a given origin:

- **`TitleBlock`** — parametric `TitleBlockFields` (part name, material,
  finish, scale, drawn-by, date, rev, units, tolerance note) laid out as a
  180×36 mm labeled grid (part-name band + 2×4 cells).
- **`RevisionTable`** — `add_revision(rev, description, date, approved_by)`
  rows under a REV/DESCRIPTION/DATE/APPD header.
- **BOM** — `BomTable::from_parts([(name, qty, material)…])` numbers rows
  1-based to match `BomBalloon { item, anchor, bubble }` (numbered bubble +
  leader + arrow on the assembly view).

## M2 — Professional output ✅

- **`LineClass`** (`sheet.rs`) — ANSI Y14.2 / ISO 128 pen classes:
  Border 0.7 mm, Visible/Section 0.5, CuttingPlane 0.6 dash-dot,
  Hidden 0.35 dashed, Hatch/Dimension/Center 0.25. Each class carries
  `weight_mm()`, `dash_pattern_mm()`, and a `dxf_layer()` name.
- **`DrawingSheet`** (`sheet.rs`) — sheet composition (A4/A3/Letter/custom
  landscape, mm, bottom-left origin, auto border): `add_projected_view`,
  `add_section_view`, `add_detail_view`, `add_annotation(rd, class, offset)`,
  and corner-anchored `add_title_block` / `add_revision_table` /
  `add_bom_table`.
- **PDF export** (`pdf.rs`) — `sheet_to_pdf(&DrawingSheet) -> Vec<u8>`.
  Dependency-free vector PDF 1.4: per-class stroke width and dash pattern,
  Helvetica text (WinAnsi; ⌀/°/± mapped), filled arrowheads. **Deterministic
  by construction** (no timestamps/IDs; primitives sorted canonically to
  neutralize upstream HashMap ordering) — same sheet, identical bytes.
- **DXF export** — `export_drawing_sheet_to_dxf[_buffer]`
  (`crates/vcad/src/export/dxf.rs`, feature `drafting`): one layer per
  `LineClass` with HIDDEN/PHANTOM/CENTER linetypes, CIRCLE/ARC entities,
  SOLID arrowheads, TEXT with alignment codes. The pre-existing
  per-view/per-section exporters are untouched.
- **Golden-file suite** —
  `crates/vcad-kernel-drafting/tests/shop_drawing_golden.rs` composes a full
  reference sheet (front view + cutting-plane callout + dimension + offset
  section A-A + detail B + BOM balloon/table + revision table + title block)
  and byte-compares against `tests/golden/shop_drawing.pdf`. Regenerate after
  intentional changes with
  `REGEN_GOLDEN=1 cargo test -p vcad-kernel-drafting --test shop_drawing_golden`.

## Follow-ups (out of scope here)

- **App/WASM wiring**: expose `DrawingSheet`, `offset_section_mesh`,
  `SectionCutLine`, `detail_callout`, title block/BOM/revision entities, and
  `sheet_to_pdf` through `vcad-kernel-wasm` (pattern: `WasmAnnotationLayer`,
  `exportProjectedViewToDxf`) and the app's Drawing mode
  (`packages/app/src/components/DrawingView.tsx`, `drawing-store.ts`), which
  currently renders `ProjectedView` JSON directly. Map `LineClass` to SVG
  stroke width/dash there.
- **Drawing persistence**: drawings are still ephemeral UI state; persisting
  a `DrawingSheet` in `.vcad` needs IR types (`vcad-ir` + `npm run ir:gen`).
- Auto view layout (third-angle arrangement), centerline inference for holes,
  aligned/half sections, and true font metrics for PDF text centering.
