# 2D Drafting

Create technical drawings with orthographic projections and dimensions for manufacturing and documentation.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `n/a` |
| Priority | `p0` |
| Effort | `n/a` (complete) |

> Verified against repo 2026-07-30. Several "future enhancements" have since shipped (DXF/PDF export, drawing sheets with title blocks, BOM with balloons) — see `crates/vcad-kernel-drafting/src/{sheet.rs,pdf.rs}`.

## Problem

Manufacturing and engineering workflows require professional technical drawings:

1. **Manufacturing** needs precise 2D drawings with dimensions and tolerances for fabrication
2. **Documentation** requires standardized views showing all features from multiple angles
3. **Quality control** depends on GD&T callouts for inspection criteria
4. **Communication** with vendors and partners requires industry-standard drawing formats

Without proper 2D drafting, users would need to export to other CAD tools for drawing creation, breaking the parametric link between 3D model and 2D documentation.

## Solution

Full 2D drafting system with orthographic/isometric projections, hidden line removal, dimensions, and GD&T annotations.

### Orthographic Projections

Standard views that project 3D geometry onto 2D planes:

| View | Direction | Shows |
|------|-----------|-------|
| Front | Looking along +Y | XZ plane (width x height) |
| Back | Looking along -Y | XZ plane (mirrored) |
| Top | Looking along -Z | XY plane (width x depth) |
| Bottom | Looking along +Z | XY plane (mirrored) |
| Right | Looking along +X | YZ plane (depth x height) |
| Left | Looking along -X | YZ plane (mirrored) |

### Isometric Projection

3D-like view for visualization with configurable azimuth and elevation angles:

- **Standard Isometric:** 30 degrees azimuth, 30 degrees elevation
- **Dimetric:** 26.57 degrees (arctan 0.5) for technical drawings

### Hidden Line Removal

Edges are classified as visible or hidden based on occlusion:

- **Visible edges:** Solid lines (not blocked by any face)
- **Hidden edges:** Dashed lines (occluded by geometry)

Edge types extracted from mesh:

| Type | Description |
|------|-------------|
| Sharp | Angle between adjacent faces exceeds threshold (default 30 degrees) |
| Silhouette | Boundary between front-facing and back-facing faces |
| Boundary | Edge with only one adjacent face (mesh boundary) |

### Dimension Types

| Dimension | Description | Use Case |
|-----------|-------------|----------|
| Linear | Horizontal, vertical, aligned, or rotated | Overall dimensions, feature locations |
| Angular | Angle between two edges or three points | Chamfers, tapers, angular features |
| Radial | Radius or diameter of arcs/circles | Holes, fillets, cylindrical features |
| Ordinate | Datum-relative coordinate dimensions | Hole patterns, feature positions from datum |

### GD&T Support

Full geometric dimensioning and tolerancing per ASME Y14.5-2018:

**Form Tolerances** (no datum required):
- Straightness, Flatness, Circularity, Cylindricity

**Profile Tolerances:**
- Profile of a Line, Profile of a Surface

**Orientation Tolerances** (require datum):
- Angularity, Perpendicularity, Parallelism

**Location Tolerances** (require datum):
- Position, Concentricity, Symmetry

**Runout Tolerances** (require datum):
- Circular Runout, Total Runout

**Material Conditions:**
- MMC (Maximum Material Condition)
- LMC (Least Material Condition)
- RFS (Regardless of Feature Size, implicit)

### Section Views

Cut through geometry to show internal features:

- **Horizontal sections:** Cut at specified Z height
- **Front sections:** Cut at specified Y depth
- **Right sections:** Cut at specified X position
- **Custom planes:** Arbitrary origin and normal
- **Hatch patterns:** Configurable spacing and angle (default 45 degrees at 2mm)

### Detail Views

Magnified regions for fine features:

- Specify center, width, height, and scale factor
- Label with letters (A, B, C, etc.)
- Automatically extracts and scales edges from parent view

## UX Details

### View Mode Toggle

| Mode | Display | Controls |
|------|---------|----------|
| 3D | Interactive viewport with orbit/pan/zoom | Mouse drag to orbit |
| 2D | SVG-based technical drawing | Pan and zoom only |

### Drawing Controls

| Control | Action |
|---------|--------|
| View Direction | Dropdown: Front, Back, Top, Bottom, Left, Right, Isometric |
| Hidden Lines | Toggle visibility of dashed hidden lines |
| Dimensions | Toggle visibility of dimension annotations |
| Zoom | Scroll wheel or slider (0.1x to 10x) |
| Pan | Click and drag in 2D mode |
| Reset View | Button to restore default zoom/pan |

### Adding Dimensions

1. Enter dimension mode
2. Select geometry (points, edges, or faces)
3. Click to place dimension at desired offset
4. Edit value if adding tolerance or override

### Adding GD&T

1. Click "Add Feature Control Frame" button
2. Select GD&T symbol from dropdown
3. Enter tolerance value
4. Optionally add diameter symbol and material condition
5. Add datum references (A, B, C) as needed
6. Click geometry to attach leader line

### Detail View Creation

1. Click "Add Detail View" button
2. Draw rectangle around region of interest
3. Enter scale factor (e.g., 2x)
4. Position the magnified view on the drawing sheet

## Implementation

### Files

| File | Purpose |
|------|---------|
| `crates/vcad-kernel-drafting/src/lib.rs` | Module exports and integration tests |
| `crates/vcad-kernel-drafting/src/types.rs` | Core types: Point2D, ViewDirection, ProjectedEdge, etc. |
| `crates/vcad-kernel-drafting/src/projection.rs` | View matrix and point projection |
| `crates/vcad-kernel-drafting/src/edge_extract.rs` | Sharp, silhouette, and boundary edge extraction |
| `crates/vcad-kernel-drafting/src/hidden_line.rs` | Occlusion testing and visibility classification |
| `crates/vcad-kernel-drafting/src/section.rs` | Plane-mesh intersection, segment chaining, hatching |
| `crates/vcad-kernel-drafting/src/detail.rs` | Detail view creation (clipping and scaling) |
| `crates/vcad-kernel-drafting/src/dimension/mod.rs` | Dimension module exports |
| `crates/vcad-kernel-drafting/src/dimension/linear.rs` | Linear dimension (horizontal, vertical, aligned) |
| `crates/vcad-kernel-drafting/src/dimension/angular.rs` | Angular dimension between edges |
| `crates/vcad-kernel-drafting/src/dimension/radial.rs` | Radius and diameter dimensions |
| `crates/vcad-kernel-drafting/src/dimension/ordinate.rs` | Datum-relative ordinate dimensions |
| `crates/vcad-kernel-drafting/src/dimension/gdt.rs` | GD&T symbols, feature control frames, datum symbols |
| `crates/vcad-kernel-drafting/src/dimension/style.rs` | Dimension styling (arrows, text placement, tolerances) |
| `crates/vcad-kernel-drafting/src/dimension/render.rs` | Rendering to lines, arcs, and text |
| `crates/vcad-kernel-drafting/src/dimension/layer.rs` | Annotation layer for collecting dimensions |
| `crates/vcad-kernel-drafting/src/dimension/geometry_ref.rs` | References to projected geometry |
| `packages/app/src/stores/drawing-store.ts` | UI state for 2D drawing mode |

### Data Structures

```typescript
// drawing-store.ts
interface DrawingState {
  viewMode: "3d" | "2d";
  viewDirection: ViewDirection;
  showHiddenLines: boolean;
  showDimensions: boolean;
  zoom: number;
  pan: { x: number; y: number };
  detailViews: DetailViewDef[];
}

type ViewDirection =
  | "front" | "back" | "top" | "bottom" | "left" | "right"
  | "isometric";
```

```rust
// types.rs
pub enum ViewDirection {
    Front,
    Back,
    Top,
    Bottom,
    Right,
    Left,
    Isometric { azimuth: f64, elevation: f64 },
}

pub struct ProjectedView {
    pub edges: Vec<ProjectedEdge>,
    pub bounds: BoundingBox2D,
    pub view_direction: ViewDirection,
}

pub struct ProjectedEdge {
    pub start: Point2D,
    pub end: Point2D,
    pub visibility: Visibility,  // Visible | Hidden
    pub edge_type: EdgeType,     // Sharp | Silhouette | Boundary
    pub depth: f64,
}
```

```rust
// gdt.rs
pub enum GdtSymbol {
    Straightness, Flatness, Circularity, Cylindricity,
    ProfileOfLine, ProfileOfSurface,
    Angularity, Perpendicularity, Parallelism,
    Position, Concentricity, Symmetry,
    CircularRunout, TotalRunout,
}

pub struct FeatureControlFrame {
    pub symbol: GdtSymbol,
    pub tolerance: f64,
    pub tolerance_is_diameter: bool,
    pub material_condition: Option<MaterialCondition>,
    pub datum_a: Option<DatumRef>,
    pub datum_b: Option<DatumRef>,
    pub datum_c: Option<DatumRef>,
    pub position: Point2D,
    pub leader_to: Option<GeometryRef>,
}
```

### Projection Algorithm

```
1. Build view matrix from view direction
   - Compute right, up, forward vectors
   - Ensure orthogonality
2. Extract edges from mesh
   - Find sharp edges (face angle > threshold)
   - Find silhouette edges (front-back face boundary)
   - Find boundary edges (single adjacent face)
3. For each edge:
   a. Project endpoints to 2D using view matrix
   b. Compute midpoint depth
   c. Test visibility against all triangles
   d. Classify as Visible or Hidden
4. Return ProjectedView with all edges and bounds
```

### Section Algorithm

```
1. For each triangle in mesh:
   - Compute vertex distances to cutting plane
   - Find edge-plane intersections (0, 1, or 2 points)
   - If 2 points, add segment to list
2. Chain segments into polylines:
   - Use tolerance-based endpoint matching
   - Build adjacency graph
   - Traverse to form continuous curves
3. Project 3D curves to 2D on section plane
4. If hatching enabled:
   - For each closed curve (boundary)
   - Generate parallel lines at specified angle/spacing
   - Clip to boundary, subtract holes
5. Return SectionView with curves and hatch lines
```

## Tasks

All tasks complete:

- [x] Implement ViewDirection and view matrix (`xs`)
- [x] Implement point projection with depth (`xs`)
- [x] Implement sharp edge extraction (`s`)
- [x] Implement silhouette edge extraction (`s`)
- [x] Implement boundary edge extraction (`xs`)
- [x] Implement hidden line removal via occlusion testing (`m`)
- [x] Implement plane-mesh intersection (`s`)
- [x] Implement segment chaining into polylines (`s`)
- [x] Implement 2D projection onto section plane (`s`)
- [x] Implement hatch line generation (`m`)
- [x] Implement linear dimensions (`s`)
- [x] Implement angular dimensions (`s`)
- [x] Implement radial dimensions (`s`)
- [x] Implement ordinate dimensions (`s`)
- [x] Implement GD&T symbols and rendering (`m`)
- [x] Implement feature control frames (`m`)
- [x] Implement datum feature symbols (`s`)
- [x] Implement detail view extraction (`s`)
- [x] Create drawing store for UI state (`s`)
- [x] Implement view direction dropdown (`xs`)
- [x] Implement hidden line toggle (`xs`)
- [x] Implement zoom/pan controls (`s`)
- [x] Implement detail view UI (`s`)

## Acceptance Criteria

- [x] Can switch between 3D and 2D view modes
- [x] Can select standard orthographic views (Front, Back, Top, Bottom, Left, Right)
- [x] Can select isometric view
- [x] Hidden lines rendered as dashed when enabled
- [x] Can toggle hidden line visibility
- [x] Can toggle dimension visibility
- [x] Sharp edges correctly identified and projected
- [x] Silhouette edges correctly identified at view boundaries
- [x] Boundary edges correctly identified on mesh boundaries
- [x] Section views correctly slice through geometry
- [x] Hatch patterns correctly fill solid regions
- [x] Holes in sections correctly exclude hatching
- [x] Linear dimensions measure correct distances
- [x] Angular dimensions measure correct angles
- [x] Radial dimensions show correct radius/diameter
- [x] Ordinate dimensions reference datum correctly
- [x] GD&T symbols render with correct Unicode characters
- [x] Feature control frames render with correct structure
- [x] Datum symbols render with triangles and letters
- [x] Material conditions display correctly (MMC, LMC)
- [x] Detail views correctly magnify selected regions
- [x] Zoom and pan work smoothly in 2D mode
- [x] View resets correctly to default state

## Future Enhancements

- [x] DXF export for CAM software and legacy CAD compatibility
- [x] PDF export for documentation and sharing (`crates/vcad-kernel-drafting/src/pdf.rs`)
- [x] Notes and balloons for part callouts and instructions (`BomBalloon` in `sheet.rs`)
- [x] BOM generation with part numbers and quantities (`BomRow` in `sheet.rs`)
- [x] Drawing sheets with title blocks and borders (`TitleBlock` in `sheet.rs`)
- [ ] Multiple views on single sheet with automatic alignment
- [ ] Centerlines and center marks for cylindrical features
- [ ] Break lines for long parts that don't fit on sheet
- [ ] Weld symbols per AWS A2.4 standard
- [ ] Surface finish symbols per ISO 1302
- [ ] Revision tables and change tracking
- [ ] Auto-dimension generation based on feature recognition
