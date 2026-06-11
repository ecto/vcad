# Sheet Metal Design

## Overview

Sheet metal design is a common manufacturing workflow for enclosures, brackets, chassis, and similar parts. Unlike solid modeling where material is added or removed from a volume, sheet metal operates on a constant-thickness shell that can be bent, folded, and formed. The key output is a **flat pattern** that can be laser/plasma cut from stock and then bent on a press brake.

## Core Concepts

### K-Factor

The K-factor represents the ratio of the neutral axis location to the material thickness. When sheet metal bends, the outer surface stretches while the inner surface compresses. The neutral axis is the theoretical plane where no stretching or compression occurs.

```
K = t / T
```

Where:
- `t` = distance from inside surface to neutral axis
- `T` = material thickness

Typical K-factor values by material:

| Material | K-Factor Range |
|----------|----------------|
| Soft aluminum | 0.33 - 0.38 |
| Mild steel | 0.40 - 0.46 |
| Stainless steel | 0.44 - 0.50 |
| Hard copper/brass | 0.35 - 0.42 |

K-factor also varies with bend radius to thickness ratio (R/T). Tighter bends shift the neutral axis inward (lower K).

### Bend Allowance (BA)

Bend allowance is the arc length of the neutral axis through a bend. This is the amount of material consumed by the bend.

```
BA = (π × A / 180) × (R + K × T)
```

Where:
- `A` = bend angle in degrees
- `R` = inside bend radius
- `K` = K-factor
- `T` = material thickness

### Bend Deduction (BD)

Bend deduction is the difference between the sum of the flange lengths measured to the theoretical sharp corner and the flat pattern length.

```
BD = 2 × (R + T) × tan(A / 2) - BA
```

This value is subtracted from the theoretical flat length to get the actual cut length.

### Outside Setback (OSSB)

Distance from the tangent point of the bend to the apex (theoretical sharp corner):

```
OSSB = (R + T) × tan(A / 2)
```

## Operations

### Base Operations

**Base Flange**
Creates the initial sheet metal body from a sketch or predefined shape.
- From sketch: Extrude a closed profile to the sheet thickness
- From predefined: L-shape, U-channel, box section with specified dimensions
- Sets default material thickness, bend radius, and K-factor for the body

**Edge Flange**
Extends a new wall from an existing edge with a bend.
- Select one or more edges
- Specify height, angle (default 90°), and optional gap
- Alignment options: material inside, material outside, bend outside

**Miter Flange**
Creates mitered corners where edge flanges meet.
- Automatically computes miter angles for clean corners
- Handles acute and obtuse corner angles

**Tab**
Creates a small flange or mounting tab.
- Sketch-driven profile on a face
- Optional bend or direct extension

### Bend Operations

**Hem**
A 180° fold at the edge of a flange, used to eliminate sharp edges and add rigidity.

| Hem Type | Description |
|----------|-------------|
| Closed | Fully folded, faces touch |
| Open | Small gap between faces |
| Teardrop | Rounded interior void |
| Rolled | Circular cross-section |

**Jog**
Two equal and opposite bends creating a Z-shaped offset.
- Offset distance
- Bend angle (typically 90°)
- Creates step between parallel planes

**Bend**
Add a bend along a sketch line on a flat face.
- Splits the face at the bend line
- Applies specified angle and radius

**Fold**
Fold along an existing edge.
- Similar to bend but uses edge geometry
- Useful for bending existing flat patterns

**Unfold / Refold**
Temporarily flatten specific bends for additional operations.
- Unfold: Flatten selected bends
- Refold: Restore bends to formed state
- Enables features that span across bends

### Relief Operations

**Bend Relief**
Small cuts at the ends of bends to prevent tearing and interference.

| Relief Type | Description |
|-------------|-------------|
| Rectangular | Square notch, simple to manufacture |
| Obround | Rounded ends, reduces stress concentration |
| Tear | No relief, material tears (low-grade applications only) |

Relief dimensions typically:
- Width: 1× material thickness (minimum)
- Depth: Inside bend radius + material thickness

**Corner Relief**
Applied where two or more bends meet at a corner.

| Relief Type | Description |
|-------------|-------------|
| Round | Circular cutout, best stress distribution |
| Square | Rectangular cutout, easier to model |
| Tear | V-notch allowing material to separate |

### Form Operations

Form tools create 3D features without cutting through the sheet.

| Operation | Description |
|-----------|-------------|
| Louver | Angled opening for ventilation |
| Emboss | Raised or recessed feature |
| Lance | Partially cut and bent tab |
| Rib | Linear stiffening bead |
| Dimple | Small raised bump |
| Punch | Standard hole patterns (round, square, slot) |

## Data Model

### IR Operations

```rust
/// Creates initial sheet metal body from sketch
SheetMetalBase {
    sketch: SketchRef,
    thickness: f64,
    bend_radius: f64,
    k_factor: f64,
    direction: ThicknessDirection, // Up, Down, Symmetric
}

/// Adds flange to existing edge
SheetMetalFlange {
    parent: OpRef,
    edges: Vec<EdgeRef>,
    height: f64,
    angle: f64,              // degrees, default 90
    gap: f64,                // gap from parent face
    alignment: FlangeAlignment,
    relief: BendReliefType,
}

/// Adds hem to edge
SheetMetalHem {
    parent: OpRef,
    edges: Vec<EdgeRef>,
    hem_type: HemType,
    length: f64,             // for open/teardrop
    gap: f64,                // for open hem
}

/// Adds jog offset
SheetMetalJog {
    parent: OpRef,
    face: FaceRef,
    line: SketchLineRef,
    offset: f64,
    angle: f64,              // default 90
    fixed_side: JogFixedSide,
}
```

### BRep Metadata

Sheet metal bodies carry additional metadata beyond standard BRep:

```rust
/// Attached to sheet metal Solid
struct SheetMetalBody {
    thickness: f64,
    default_bend_radius: f64,
    k_factor: f64,
    material: Option<MaterialId>,
}

/// Attached to cylindrical faces representing bends
struct BendInfo {
    inside_face: FaceId,
    outside_face: FaceId,
    angle: f64,
    radius: f64,
    bend_line: Line3d,       // axis of bend cylinder
    direction: BendDirection, // Up or Down
    sequence: Option<u32>,   // manufacturing order
}

/// Attached to flat faces representing flanges
struct FlangeInfo {
    face: FaceId,
    thickness_direction: Vec3,
    connected_bends: Vec<BendId>,
    is_reference: bool,      // true for the base face
}
```

## Unfolding Algorithm

Converting a bent sheet metal body to a flat pattern.

### Phase 1: Sheet Metal Recognition

For parts imported from STEP or created as generic solids:

1. **Thickness detection**: Find pairs of offset planar faces
2. **Bend detection**: Identify cylindrical faces connecting face pairs
3. **Build Adjacency-Angle Graph (AAG)**: Nodes are flat faces, edges are bends with angle annotations

### Phase 2: Build Unfolding Tree

1. Select reference face (typically largest flat face)
2. BFS traversal through bends to visit all faces
3. Build tree structure: each node is a face, edges are bends to traverse
4. Handle loops by selecting minimum-angle unfold path

### Phase 3: Compute Transformations

For each face in the tree, compute the transformation to flatten:

```
T_flatten = T_center × R_unbend × T_to_origin
```

Where:
- `T_to_origin` translates bend axis to origin
- `R_unbend` rotates by negative bend angle around the axis
- `T_center` translates back and applies parent transformation

Apply bend deduction by adjusting face positions based on material consumption.

### Phase 4: Flatten Cylindrical Faces

Bend faces are developable surfaces that unwrap to rectangles:

1. Extract bend cylinder parameters (radius, angle, length)
2. Compute arc length: `L = R × θ`
3. Create rectangular face with width = arc length
4. Map any holes or features from 3D to 2D coordinates

### Phase 5: Assemble Flat Pattern

1. Apply transformations to all faces
2. Project to XY plane (Z = 0)
3. Merge coplanar faces
4. Extract outer boundary and holes
5. Generate bend line annotations

### Flat Pattern Output

```rust
struct FlatPattern {
    /// Outer boundary as closed polyline
    outer_boundary: Polyline,

    /// Interior cutouts
    holes: Vec<Polyline>,

    /// Bend lines with annotations
    bend_lines: Vec<BendLineAnnotation>,

    /// Corner relief cutouts (already in holes)
    corner_reliefs: Vec<CornerReliefInfo>,

    /// Bounding box for nesting
    bbox: BoundingBox2d,

    /// Total material area
    area: f64,
}

struct BendLineAnnotation {
    line: Line2d,
    direction: BendDirection,  // Up or Down
    angle: f64,
    radius: f64,
    sequence: Option<u32>,
}
```

## Material Database

### K-Factor Tables

K-factor varies with material and R/T ratio:

| R/T Ratio | Soft Al | Mild Steel | Stainless |
|-----------|---------|------------|-----------|
| 0.5 | 0.33 | 0.38 | 0.42 |
| 1.0 | 0.35 | 0.40 | 0.44 |
| 2.0 | 0.37 | 0.43 | 0.47 |
| 3.0 | 0.38 | 0.45 | 0.49 |

### Standard Gauge Tables

**Steel (USS Gauge)**

| Gauge | Thickness (mm) | Thickness (in) |
|-------|----------------|----------------|
| 10 | 3.416 | 0.1345 |
| 12 | 2.657 | 0.1046 |
| 14 | 1.897 | 0.0747 |
| 16 | 1.519 | 0.0598 |
| 18 | 1.214 | 0.0478 |
| 20 | 0.912 | 0.0359 |
| 22 | 0.759 | 0.0299 |
| 24 | 0.607 | 0.0239 |

**Aluminum (Brown & Sharpe)**

| Gauge | Thickness (mm) | Thickness (in) |
|-------|----------------|----------------|
| 10 | 2.588 | 0.1019 |
| 12 | 2.052 | 0.0808 |
| 14 | 1.628 | 0.0641 |
| 16 | 1.291 | 0.0508 |
| 18 | 1.024 | 0.0403 |
| 20 | 0.812 | 0.0320 |

### Minimum Bend Radius

Expressed as multiplier of material thickness:

| Material | Min R/T (soft) | Min R/T (hard) |
|----------|----------------|----------------|
| Aluminum 1100 | 0 | 1 |
| Aluminum 6061-T6 | 1 | 3 |
| Mild Steel | 0.5 | 1 |
| Stainless 304 | 0.5 | 2 |
| Brass | 0 | 1 |

## DXF Export

Flat patterns export to DXF with standardized layers for manufacturing:

| Layer Name | Color | Description |
|------------|-------|-------------|
| CUT | Red (1) | Cut lines - outer boundary and holes |
| BEND_UP | Blue (5) | Bend up (away from operator) |
| BEND_DOWN | Cyan (4) | Bend down (toward operator) |
| FORM | Yellow (2) | Form tool outlines |
| ETCH | Green (3) | Part numbers, annotations |
| GRAIN | Magenta (6) | Grain direction indicator |

Export options:
- Units: mm or inches
- Decimal precision: configurable
- Polyline vs line segments
- Include bend table as text block

## Exporting to fab services (SendCutSend)

vcad can target online sheet-metal services directly. The reference
integration is **SendCutSend** via the built-in `sendcutsend` shop profile.

### Merged-silhouette DXF

The DXF returned by `sheet_metal_unfold` (the `dxf` field of chain
evaluation) is a **fab-ready merged silhouette**, not a per-panel layered
drawing:

- One closed exterior `LWPOLYLINE` plus hole polylines on the `CUT` layer.
- Bend centerlines as `LINE` entities with the `DASHED` linetype on
  `BEND_UP` / `BEND_DOWN`, recentered on the bend-allowance midline.
- No dimensions, no text, no annotations — exactly what upload-and-quote
  services expect.

If the flat pattern is disconnected (panels that don't share material),
evaluation returns an error instead of a DXF.

### Bend relief

The `BendRelief` chain op (`bend_relief` in `sheet_metal_create`) cuts
relief notches at the ends of every bend whose parent material sits in the
deformation zone. It is parametric — the notches appear in the 3D body, the
flat pattern, and the DXF. Defaults: width `max(1.5·t, 1 mm)`, depth
`R + t`, or the shop's published per-thickness relief depth when a shop
profile is active. The DFM checker reports missing reliefs as the
auto-fixable `sheet.bend_relief` warning (`MissingBendRelief`).

### The `sendcutsend` shop profile

Setting `shop_profile: "sendcutsend"` on the base flange resolves every
bend's inside radius and K-factor from SCS's published data, so vcad's flat
pattern matches what their tooling actually forms.

- **Data source:** sendcutsend.com bending calculator (per-material
  K-factor, effective inside radius @90°, die width, min flange, relief
  depth) and their processing min/max chart (max bend length, part size
  limits). Retrieved 2026-06-10; source URLs and caveats are recorded in
  the catalog itself (`getSheetMetalShopCatalog("sendcutsend")` /
  `sheet_metal_bend_table` with `shop_profile`).
- **Fixed radii:** SCS air-bends with fixed metric tooling — the inside
  radius is *not* customer-selectable. Custom `radius` values are rejected
  with an error naming the fixed radius; omit `radius` to get it
  automatically. `sheet_metal_check` flags off-catalog radii as the
  `sheet.bend_radius_fixed` error (`BendRadiusNotFixed`).
- **Material coverage:** 5052 aluminum, mild/galvanized steel, 304/316
  stainless, 4130 chromoly, brass, copper, polycarbonate, Ti grade 2.
  **6061-T6 is not bendable** (SCS cuts it but does not offer bending) —
  use 5052 for formed aluminum parts.

### Two upload paths

| | DXF (flat pattern) | STEP (folded body) |
|---|---|---|
| Upload | merged-silhouette DXF from `sheet_metal_unfold` | `export_cad` with a `.step` filename |
| Bend angles | **not carried by DXF** — entered manually in the fab's UI | auto-detected from the folded geometry, zero data entry |
| Requirement | flat pattern must use the shop's radii/K to be dimensionally right | shop profile must be set at model time so radii/K match their tooling |

For SendCutSend the STEP path is the lowest-friction option: model with
`shop_profile: "sendcutsend"`, run `sheet_metal_check`, then export the
folded body as STEP.

## Nesting

Arranging multiple flat patterns on stock sheet for minimal waste.

### Problem Definition

2D bin packing is NP-hard. For rectangular parts, there are polynomial-time approximation algorithms. For irregular shapes, heuristics are required.

### Algorithm: First-Fit Decreasing + Bottom-Left

1. Sort parts by area (largest first)
2. For each part:
   - Try all valid rotations (0°, 90°, 180°, 270°)
   - Find bottom-left-most valid position
   - Place part if it fits, else try next sheet
3. Report material utilization

### No-Fit Polygon (NFP)

For irregular shapes:

1. Compute NFP between each pair of shapes
2. NFP defines region where reference point cannot be placed
3. Valid placements are outside all NFPs and inside stock boundary
4. Use genetic algorithm or simulated annealing to optimize

### Parameters

```rust
struct NestingConfig {
    /// Stock sheet dimensions
    sheet_width: f64,
    sheet_height: f64,

    /// Minimum spacing between parts
    part_spacing: f64,

    /// Minimum margin from sheet edge
    edge_margin: f64,

    /// Allowed rotation increments (degrees)
    rotation_step: f64,

    /// Allow parts to share cut lines
    common_cut: bool,

    /// Grain direction constraint
    grain_direction: Option<Vec2>,
}
```

## Bend Sequence Optimization

Determining the order of bends for press brake manufacturing.

### Constraints

1. **Collision avoidance**: Part must not collide with press brake tooling
2. **Back gauge accessibility**: Positioning surface must be reachable
3. **Part stability**: Part should rest stably for each bend

### Heuristics

**Outer-before-inner rule**: Bends farther from center should generally come first to avoid collisions with the press brake.

**Short-flange-first**: Shorter flanges bent first tend to cause fewer collisions.

**Acute-before-obtuse**: Acute angles create less interference.

### Algorithm

1. Build interference graph: edges connect bends that cannot be sequential
2. Find valid topological orderings
3. Score orderings by:
   - Number of part flips required
   - Tool changes required
   - Back gauge repositioning
4. Return optimal sequence

### Output

```rust
struct BendSequence {
    steps: Vec<BendStep>,
    total_setups: u32,
    tool_changes: u32,
}

struct BendStep {
    bend_id: BendId,
    bend_line: Line2d,
    angle: f64,
    direction: BendDirection,
    tool: ToolId,
    back_gauge_position: f64,
    part_orientation: Transform2d,
}
```

## Implementation Phases

### Phase 1: Foundation (4-6 weeks)

- Sheet metal data model in `vcad-kernel`
- `SheetMetalBase` operation with sketch extrusion
- `SheetMetalFlange` operation with simple edge detection
- Basic unfold for single-bend parts
- Unit tests for bend allowance calculations

### Phase 2: Core Operations (4-6 weeks)

- Hem operation (all types)
- Jog operation
- Miter flange with corner handling
- Multi-bend unfolding algorithm
- Bend relief (rectangular, obround)
- Corner relief (round, square)

### Phase 3: Advanced Features (4-6 weeks)

- Form tools (louver, emboss, lance)
- Sheet metal recognition from imported solids
- Self-intersection detection during unfold
- Bend table generation
- K-factor lookup tables

### Phase 4: Manufacturing Output (3-4 weeks)

- DXF export with standard layers
- Bend sequence optimization
- Basic rectangular nesting
- Material utilization reporting

### Phase 5: UI Integration (3-4 weeks)

- Sheet metal mode in app
- Material/gauge selector panel
- Interactive flange creation
- Flat pattern preview
- Bend table display

### Phase 6: Polish (2-3 weeks)

- GPU-accelerated nesting (wgpu compute)
- NFP-based irregular nesting
- Edge cases and error handling
- Performance optimization
- Documentation and examples

## References

- Machinery's Handbook - Bend Allowance Tables
- ANSI Y14.5 - Dimensioning and Tolerancing
- DIN 6935 - Cold bending of flat steel products
- Press Brake Technology by Steve Benson
