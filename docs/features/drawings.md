# 2D Technical Drawings

## Status

| Field | Value |
|-------|-------|
| State | `in-progress` |
| Owner | `unassigned` |
| Priority | `p2` |
| Effort | `l` |

> Verified against repo 2026-07-30. Parts of this spec have shipped in `vcad-kernel-drafting`: title blocks, BOM with balloons (`sheet.rs`), PDF export (`pdf.rs`), offset sections, and DXF output. Weld symbols, surface finish symbols, auxiliary views, and broken views remain unbuilt.

## Overview

Professionals need complete documentation for manufacturing, inspection, and assembly. vcad already has foundational 2D drafting capabilities in `vcad-kernel-drafting`. This specification extends those capabilities to support full production-ready technical drawings with comprehensive GD&T, weld symbols, surface finish annotations, and standards-compliant output.

## Current Capabilities

The existing `vcad-kernel-drafting` crate provides:

- **Orthographic projection** - 6 standard views (Front, Back, Top, Bottom, Left, Right)
- **Pictorial views** - Isometric and dimetric projections
- **Hidden line removal** - Computed visibility for overlapping geometry
- **Section views** - Cross-sections with automatic hatching
- **Detail views** - Magnified regions with circular boundaries
- **Basic GD&T** - `FeatureControlFrame` with 14 characteristic symbols
- **Dimensions** - Linear, angular, radial, and ordinate
- **DXF export** - 2D drawing output

## GD&T Extensions (ASME Y14.5-2018 / ISO 1101)

### Feature Control Frame

The feature control frame (FCF) is the primary vehicle for communicating geometric tolerances. Extensions beyond the current implementation:

```
┌─────────────┬───────────┬─────────┬─────────┬─────────┐
│  Symbol     │ Tolerance │ Primary │ Secondary│ Tertiary│
├─────────────┼───────────┼─────────┼─────────┼─────────┤
│  ⌖ (Position)│ ⌀0.05 Ⓜ │   A    │   B Ⓜ   │   C    │
├─────────────┼───────────┼─────────┼─────────┼─────────┤
│             │ ⌀0.10 Ⓜ │   A    │   B Ⓜ   │         │  ← Composite row
└─────────────┴───────────┴─────────┴─────────┴─────────┘
```

#### Tolerance Zone Shapes

| Shape | Symbol | Description |
|-------|--------|-------------|
| Width | (none) | Two parallel planes |
| Diameter | ⌀ | Cylindrical zone |
| Spherical Diameter | S⌀ | Spherical zone |

#### Material Condition Modifiers

| Modifier | Symbol | Application |
|----------|--------|-------------|
| Maximum Material Condition | Ⓜ | Bonus tolerance at MMC |
| Least Material Condition | Ⓛ | Bonus tolerance at LMC |
| Regardless of Feature Size | (none) | No bonus tolerance |

#### Additional Modifiers

- **Projected tolerance zone** (Ⓟ) - Extends tolerance zone beyond feature
- **Statistical tolerance** (ST) - Statistical process control
- **Tangent plane** (Ⓣ) - Tolerance applied to tangent plane
- **Free state** (Ⓕ) - Non-rigid part in free state
- **Unequally disposed** (Ⓤ) - Asymmetric tolerance distribution

#### Datum References

```rust
struct DatumReference {
    label: char,                    // A, B, C, etc.
    modifier: Option<MaterialCondition>,
    translation: bool,              // Movable datum
    dof_constraints: Vec<DegreeOfFreedom>,
}

enum DegreeOfFreedom {
    TranslationX, TranslationY, TranslationZ,
    RotationX, RotationY, RotationZ,
}
```

### All 14 Characteristic Symbols

#### Form Tolerances (No datum reference)

| Symbol | Characteristic | Controls |
|--------|---------------|----------|
| ─ | Straightness | Line element deviation |
| ▭ | Flatness | Surface planarity |
| ○ | Circularity | Cross-section roundness |
| ⌭ | Cylindricity | Combined circularity and straightness |

#### Profile Tolerances

| Symbol | Characteristic | Controls |
|--------|---------------|----------|
| ⌒ | Profile of a Line | 2D curve tolerance |
| ⌓ | Profile of a Surface | 3D surface tolerance |

#### Orientation Tolerances (Require datum)

| Symbol | Characteristic | Controls |
|--------|---------------|----------|
| ∠ | Angularity | Angle relative to datum |
| ⊥ | Perpendicularity | 90° orientation |
| ∥ | Parallelism | Parallel orientation |

#### Location Tolerances (Require datum)

| Symbol | Characteristic | Controls |
|--------|---------------|----------|
| ⌖ | Position | True position location |
| ◎ | Concentricity | Axis alignment (deprecated in Y14.5-2018) |
| ≡ | Symmetry | Median plane alignment |

#### Runout Tolerances (Require datum axis)

| Symbol | Characteristic | Controls |
|--------|---------------|----------|
| ↗ | Circular Runout | Single cross-section deviation |
| ↗↗ | Total Runout | Full surface deviation |

## Weld Symbols (AWS A2.4:2020)

### Symbol Structure

```
                    Finish symbol
                         │
           ┌─────────────┴─────────────┐
           │                           │
    Contour symbol                     │
           │                           │
    ┌──────┴──────┐          ┌────────┴────────┐
    │   Size      │          │   Other side    │
    │   ────      │          │   specification │
    │   │  │      │          │                 │
    ▼   ▼  ▼      ▼          │                 │
   ╱──────────────╲──────────┼────────────────╱
  ╱                ╲         │               ╱
 ╱   Arrow side     ╲        │  Reference   ╱
╱    specification   ╲───────┴─────────────╱
                      ╲                   ╱
                       ╲     Tail        ╱
                        ╲   (process,   ╱
                         ╲  spec)      ╱
                          ╲          ╱
                           ╲        ╱
                            ▼      ╱
                          Arrow
```

### Symbol Components

```rust
struct WeldSymbol {
    arrow_side: Option<WeldSpec>,
    other_side: Option<WeldSpec>,
    tail: Option<String>,           // Process/specification reference
    field_weld: bool,               // Weld performed in field
    all_around: bool,               // Weld extends around joint
    melt_through: bool,             // Complete joint penetration
    backing: Option<BackingSpec>,
    spacer: Option<SpacerSpec>,
}

struct WeldSpec {
    weld_type: WeldType,
    size: Option<f64>,
    length: Option<f64>,
    pitch: Option<f64>,             // Center-to-center spacing
    count: Option<u32>,             // Number of welds
    contour: Option<WeldContour>,
    finish_method: Option<FinishMethod>,
}
```

### Weld Types

#### Groove Welds

| Type | Symbol | Description |
|------|--------|-------------|
| Square | ∥ | No edge preparation |
| V | V | V-shaped groove |
| Bevel | ⌐ | Single-sided bevel |
| U | U | U-shaped groove |
| J | J | J-shaped groove |
| Flare-V | )(  | Two curved surfaces |
| Flare-Bevel | ) | One curved, one flat |

#### Other Weld Types

| Type | Symbol | Description |
|------|--------|-------------|
| Fillet | △ | Right-angle joint |
| Plug | ◇ | Hole fill weld |
| Slot | ▭ | Elongated hole fill |
| Spot | ○ | Resistance spot |
| Seam | ═ | Continuous seam |
| Stud | ╥ | Stud attachment |
| Edge | ⌐ | Edge joint |

### Weld Attributes

```rust
enum WeldContour {
    Flush,      // Flat surface
    Convex,     // Raised surface
    Concave,    // Recessed surface
}

enum FinishMethod {
    C,  // Chipping
    G,  // Grinding
    H,  // Hammering
    M,  // Machining
    R,  // Rolling
    U,  // Unspecified
}
```

## Surface Finish Symbols (ISO 1302 / ISO 21920)

### Symbol Types

```
    Basic               Removal Required      Removal Prohibited
    (any process)       (machining)           (as-cast, etc.)

       ╱                    ╱                      ╱
      ╱                    ╱                      ╱
     ╱                    ╱━━                    ╱○
    ╱                    ╱                      ╱
```

### Surface Texture Parameters

#### Roughness Parameters (R)

| Parameter | Description |
|-----------|-------------|
| Ra | Arithmetic mean deviation |
| Rz | Maximum height |
| Rq | Root mean square deviation |
| Rp | Maximum peak height |
| Rv | Maximum valley depth |
| Rt | Total height |
| Rc | Mean height of profile elements |

#### Waviness Parameters (W)

| Parameter | Description |
|-----------|-------------|
| Wa | Arithmetic mean waviness |
| Wz | Maximum waviness height |
| Wq | Root mean square waviness |

### Surface Finish Specification

```rust
struct SurfaceFinish {
    symbol_type: SurfaceSymbolType,
    parameter: SurfaceParameter,
    value: f64,
    upper_limit: Option<f64>,       // For range specification
    transmission_band: Option<(f64, f64)>,  // λs, λc cutoff wavelengths
    evaluation_length: Option<f64>,
    acceptance_rule: AcceptanceRule,
    lay_direction: Option<LayDirection>,
    manufacturing_method: Option<String>,
}

enum AcceptanceRule {
    SixteenPercent,  // 16% rule (default)
    Maximum,         // Max rule
}

enum LayDirection {
    Parallel,        // = to boundary
    Perpendicular,   // ⊥ to boundary
    Crossed,         // X pattern
    Multidirectional,// M pattern
    Circular,        // C concentric
    Radial,          // R radiating
    Particulate,     // P non-directional
}
```

## Annotations

### Balloons

Balloons link parts in the drawing to the bill of materials.

```rust
struct Balloon {
    shape: BalloonShape,
    item_number: String,
    quantity: Option<u32>,          // For quantity callout
    leader: LeaderLine,
}

enum BalloonShape {
    Circle,
    Hexagon,
    Triangle,
    Square,
    Diamond,
    Flag,
    Split { top: String, bottom: String },
}

struct LeaderLine {
    points: Vec<Point2D>,
    terminator: LeaderTerminator,
    shoulder: bool,                 // Horizontal segment before text
}

enum LeaderTerminator {
    Arrow,
    Dot,
    Open,
    None,
}
```

### Bill of Materials

```rust
struct BillOfMaterials {
    columns: Vec<BomColumn>,
    entries: Vec<BomEntry>,
    structured: bool,               // Indented/structured BOM
    sort_order: BomSortOrder,
}

enum BomColumn {
    ItemNumber,
    PartNumber,
    Description,
    Quantity,
    Material,
    Mass,
    Vendor,
    Notes,
    Custom(String),
}

struct BomEntry {
    item_number: String,
    part_id: PartId,                // Link to document part
    values: HashMap<BomColumn, String>,
    indent_level: u32,              // For structured BOM
    children: Vec<BomEntry>,
}
```

Features:
- **Auto-extraction** from assembly structure
- **Balloon linking** - Automatic item number synchronization
- **Structured BOM** - Hierarchical indentation for sub-assemblies
- **Quantity roll-up** - Aggregates identical parts

### Title Block Templates

```rust
struct TitleBlockTemplate {
    name: String,
    fields: Vec<TitleBlockField>,
    border: BorderSpec,
    zones: Option<ZoneSpec>,        // A1, B2, etc.
}

struct TitleBlockField {
    name: String,
    position: Rectangle,
    source: FieldSource,
    format: Option<String>,         // Date format, number format, etc.
    font: FontSpec,
}

enum FieldSource {
    Manual(String),
    DocumentProperty(String),       // title, author, etc.
    Date(DateType),
    User,                           // Current user
    Computed(ComputedField),
    Revision,
}

enum ComputedField {
    Scale,
    SheetNumber,
    TotalSheets,
    Mass,
    Volume,
    SurfaceArea,
    BoundingBox,
}
```

### Revision Table

```rust
struct RevisionTable {
    entries: Vec<RevisionEntry>,
    position: TablePosition,
}

struct RevisionEntry {
    revision: String,               // A, B, C or 1, 2, 3
    date: String,
    description: String,
    zone: Option<String>,           // Drawing zone reference
    approved_by: Option<String>,
}
```

## View Types

### Auxiliary Views

Auxiliary views show surfaces that are not parallel to any principal plane.

```rust
struct AuxiliaryView {
    reference_edge: EdgeId,         // Edge defining view direction
    angle: f64,                     // Rotation from reference
    scale: f64,
    label: String,                  // "VIEW A-A"
    projection_plane: Plane,
}
```

The view direction is computed perpendicular to the reference edge in the plane of the parent view.

### Section Types

```rust
enum SectionType {
    Full,                           // Complete cross-section
    Half,                           // Half section (symmetrical parts)
    Offset {                        // Stepped section
        segments: Vec<SectionSegment>,
    },
    Aligned {                       // Radial features rotated to plane
        rotation_angle: f64,
    },
    Removed {                       // Section shown separately
        location: Point2D,
    },
    Revolved {                      // Section rotated in place
        center: Point2D,
    },
    BrokenOut {                     // Local section
        boundary: Vec<Point2D>,     // Spline boundary
        depth: f64,
    },
}

struct SectionSegment {
    start: Point2D,
    end: Point2D,
}
```

### Broken Views

For representing long parts in limited space.

```rust
struct BrokenView {
    break_locations: Vec<BreakLocation>,
    omitted_length: f64,            // Actual length removed
}

struct BreakLocation {
    position: f64,                  // Position along part length
    style: BreakStyle,
}

enum BreakStyle {
    Freehand,                       // Zigzag line
    SCurve,                         // For cylindrical parts
    Straight,                       // Simple gap
}
```

## Standards Compliance

### Drawing Standards

```rust
enum DrawingStandard {
    ASME {                          // American Society of Mechanical Engineers
        projection: ProjectionType::ThirdAngle,
        dimensioning: DimensionStandard::Y14_5,
    },
    ISO {                           // International Organization for Standardization
        projection: ProjectionType::FirstAngle,
        dimensioning: DimensionStandard::GPS,
    },
    DIN,                            // German Institute for Standardization
    JIS,                            // Japanese Industrial Standards
    GB,                             // Chinese National Standards
}

enum ProjectionType {
    FirstAngle,                     // ISO default
    ThirdAngle,                     // ASME default
}
```

### Projection Angle Symbols

```
Third Angle (ASME)          First Angle (ISO)

    ┌───┐                       ┌───┐
    │   │                       │   │
    └───┘                       └───┘
      ╱╲                          ╲╱
     ╱  ╲                         ╱╲
    ╱ ○  ╲                       ╱ ○ ╲
```

### Dimension Placement

```rust
struct DimensionPlacement {
    scheme: DimensionScheme,
    collision_avoidance: bool,
    stagger_offset: f64,            // For crowded dimensions
    min_spacing: f64,
}

enum DimensionScheme {
    Baseline {                      // All from common baseline
        origin: Point2D,
    },
    Chain,                          // End-to-end
    Ordinate {                      // Coordinate dimensions
        origin: Point2D,
    },
    Direct,                         // Point-to-point
}
```

## File Formats

### PDF Export

```rust
struct PdfExportOptions {
    layers: Vec<PdfLayer>,
    font_embedding: FontEmbedding,
    vector_output: bool,            // vs rasterized
    pdf_version: PdfVersion,        // 1.5+ for layer support
    color_profile: Option<ColorProfile>,
}

enum PdfLayer {
    Visible,
    Hidden,
    Dimensions,
    GDT,
    Notes,
    TitleBlock,
    Hatching,
    Construction,
}

enum FontEmbedding {
    Full,
    Subset,
    None,
}
```

### Enhanced DXF Export

```rust
struct DxfExportOptions {
    version: DxfVersion,            // R2018 recommended
    native_dimensions: bool,        // Use DIMENSION entities
    block_definitions: bool,        // Symbols as blocks
    dim_styles: Vec<DimStyle>,
    layer_mapping: HashMap<DrawingLayer, String>,
}
```

Improvements over current export:
- **Native DIMENSION entities** - Editable in CAD software
- **Block definitions** - Symbols as reusable blocks
- **DimStyle support** - Dimension styling per standard
- **Full layer hierarchy** - Proper layer organization

## Implementation Phases

### Phase 1: Foundation (4-6 weeks)

- Extended FCF with composite tolerances and all modifiers
- Complete annotation system (balloons, leaders, notes)
- Standards configuration framework
- Enhanced dimension placement algorithm

### Phase 2: Symbols and Views (4-6 weeks)

- Weld symbol rendering and specification
- Surface finish symbol system
- Auxiliary view generation
- Additional section types (offset, aligned, broken-out)
- Broken view support

### Phase 3: Automation and Export (4-6 weeks)

- Bill of materials auto-extraction
- Title block template system
- Revision tracking
- PDF export with layers
- Enhanced DXF with native dimensions

### Phase 4: Polish (2-4 weeks)

- App UI integration (drawing mode panels)
- MCP tools for programmatic drawing creation
- Validation against standards
- Documentation and examples

## API Design

### Rust API

```rust
// Create a drawing sheet
let mut sheet = DrawingSheet::new(PaperSize::A2, DrawingStandard::ASME);

// Add views
let front = sheet.add_orthographic_view(solid, ViewDirection::Front, scale: 1.0);
let section = sheet.add_section_view(solid, &cutting_plane, SectionType::Full);
let detail = sheet.add_detail_view(&front, center, radius, scale: 2.0);

// Add dimensions
sheet.add_linear_dimension(&front, edge1, edge2, offset: 10.0);
sheet.add_diameter_dimension(&front, circle_edge);

// Add GD&T
let fcf = FeatureControlFrame::new(Characteristic::Position)
    .tolerance(0.05, ToleranceZone::Diameter)
    .modifier(MaterialCondition::MMC)
    .datum("A")
    .datum_with_modifier("B", MaterialCondition::MMC);
sheet.add_fcf(&front, face_id, fcf);

// Add weld symbol
let weld = WeldSymbol::new()
    .arrow_side(WeldType::Fillet, size: 6.0)
    .all_around(true);
sheet.add_weld_symbol(&front, edge_id, weld);

// Export
sheet.export_pdf("drawing.pdf", &PdfExportOptions::default())?;
sheet.export_dxf("drawing.dxf", &DxfExportOptions::default())?;
```

### MCP Tools

```json
{
  "name": "create_drawing",
  "parameters": {
    "document_id": "uuid",
    "paper_size": "A2",
    "standard": "ASME",
    "views": [
      { "type": "orthographic", "direction": "front", "scale": 1.0 },
      { "type": "section", "cutting_plane": [...], "label": "A-A" }
    ]
  }
}

{
  "name": "add_gdt",
  "parameters": {
    "drawing_id": "uuid",
    "view_id": "uuid",
    "characteristic": "position",
    "tolerance": 0.05,
    "zone": "diameter",
    "modifiers": ["MMC"],
    "datums": ["A", "B|MMC", "C"]
  }
}
```

## References

- ASME Y14.5-2018: Dimensioning and Tolerancing
- ASME Y14.41-2019: Digital Product Definition Data Practices
- ISO 1101:2017: Geometrical tolerancing
- ISO 1302:2002: Surface texture indication
- ISO 21920-1:2021: Surface texture: Profile
- AWS A2.4:2020: Standard Symbols for Welding
- ISO 2553:2019: Welding and allied processes - Symbolic representation
