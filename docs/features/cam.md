# CAM (Computer-Aided Manufacturing)

## Overview

CAM is the most critical missing feature in vcad. It blocks users who design parts with the intent to manufacture them physically. Without CAM, users must export to external software (Fusion 360 CAM, Mastercam, etc.) to generate toolpaths and G-code.

This document specifies the architecture and implementation plan for native CAM in vcad.

## Architecture

### New Crates

```
crates/
├── vcad-kernel-cam/       # Toolpath generation algorithms
├── vcad-kernel-gcode/     # G-code generation and post-processors
└── vcad-kernel-stocksim/  # Stock simulation (SDF octree)
```

### Data Flow

```
BRep Solid
    │
    ▼
Geometry Analysis (feature recognition, stock definition)
    │
    ▼
Toolpath Generation (per-operation)
    │
    ▼
Toolpath IR (CL data - cutter location)
    │
    ▼
Post-Processor (machine-specific)
    │
    ▼
G-code Output
```

## Core Data Structures

### Toolpath IR (Cutter Location Data)

The intermediate representation for toolpaths uses tagged segments:

```rust
pub enum ToolpathSegment {
    Rapid { to: Point3 },
    Linear { to: Point3, feed: f64 },
    Arc { to: Point3, center: Point3, plane: ArcPlane, dir: ArcDir, feed: f64 },
    Helix { to: Point3, center: Point3, pitch: f64, dir: ArcDir, feed: f64 },
    Orientation { axis: Vec3 },  // 5-axis tool orientation
    Dwell { seconds: f64 },
    Spindle { rpm: f64, dir: SpindleDir },
    Coolant { mode: CoolantMode },
}

pub enum ArcPlane { XY, XZ, YZ }
pub enum ArcDir { CW, CCW }
pub enum SpindleDir { CW, CCW, Off }
pub enum CoolantMode { Flood, Mist, Off }
```

### Tool Definitions

```rust
pub enum Tool {
    FlatEndMill { diameter: f64, flute_length: f64, flutes: u8 },
    BallEndMill { diameter: f64, flute_length: f64, flutes: u8 },
    BullEndMill { diameter: f64, corner_radius: f64, flute_length: f64, flutes: u8 },
    ChamferMill { diameter: f64, angle: f64 },
    Drill { diameter: f64, point_angle: f64 },
    FaceMill { diameter: f64, inserts: u8 },
}

pub struct ToolEntry {
    pub number: u32,
    pub tool: Tool,
    pub holder_diameter: f64,
    pub holder_length: f64,
    pub description: String,
}
```

### CAM Operations

```rust
pub enum CamOperation {
    // 2.5D Operations
    Face { depth: f64, stepover: f64 },
    Pocket2D { contour: Contour2D, depth: f64, stepover: f64, stepdown: f64 },
    Contour2D { contour: Contour2D, depth: f64, offset: f64, tabs: Vec<Tab> },
    AdaptiveClearing { region: Region2D, depth: f64, optimal_load: f64 },

    // 3D Operations
    Parallel3D { angle: f64, stepover: f64, tolerance: f64 },
    Waterline3D { stepdown: f64, tolerance: f64 },
    Scallop3D { stepover: f64, tolerance: f64 },
    Pencil3D { tolerance: f64 },

    // Drilling
    Drill { holes: Vec<DrillHole>, cycle: DrillCycle },
}

pub enum DrillCycle {
    Standard,
    Peck { peck_depth: f64 },
    ChipBreak { peck_depth: f64 },
    Tapping { pitch: f64 },
    Boring { dwell: f64 },
}
```

## Algorithms

### Adaptive Clearing

Adaptive clearing maintains constant tool engagement (radial immersion) for consistent chip load and extended tool life:

- **Constant engagement**: Target radial engagement (e.g., 40% tool diameter)
- **Trochoidal motion**: Spiral entry, arc transitions to avoid full-width cuts
- **Slicing**: Decompose pocket into engagement-limited slices

Reference: HSMWorks/Fusion 360 adaptive, Mastercam Dynamic Motion.

### Drop-Cutter (3D Surfacing)

For 3D toolpaths, drop-cutter calculates the Z-height where a tool contacts the surface:

```
For each XY sample point:
    Drop tool vertically until contact
    Record contact Z as toolpath height
    Handle multiple surface types: triangles, BRep faces
```

Port algorithms from OpenCAMLib (BSD license):
- `DropCutter` for flat/ball/bull endmills
- `CompositeCutter` for complex tool shapes
- Triangle and parametric surface intersection

### 2D Pocketing

Use Clipper2 (via `geo-clipper` crate) for polygon offsetting:

1. Inset boundary by tool radius
2. Generate parallel passes with stepover
3. Connect passes with smooth transitions
4. Handle islands (positive contours within pocket)

### Stock Simulation

SDF (Signed Distance Field) octree representation:

```rust
pub struct StockOctree {
    root: OctreeNode,
    resolution: f64,  // minimum cell size
}

pub enum OctreeNode {
    Leaf { sdf: f64 },  // signed distance to surface
    Branch { children: Box<[OctreeNode; 8]> },
}
```

Material removal: subtract swept tool volume from SDF.
Visualization: marching cubes to extract isosurface mesh.

## Open Source Libraries

| Library | Purpose | Integration |
|---------|---------|-------------|
| Clipper2 (`geo-clipper`) | 2D polygon offsetting | Direct Rust binding |
| OpenCAMLib | Drop-cutter reference | Port algorithms (C++ → Rust) |
| phyz-collision | Collision detection (GJK/EPA) | Stock-tool intersection |
| tang-la | Linear algebra | Already in vcad |

## Post-Processors

Post-processors convert toolpath IR to machine-specific G-code dialects.

### Supported Machines

| Post | Target | Notes |
|------|--------|-------|
| GRBL | Hobby CNC (Shapeoko, X-Carve, etc.) | Most common, priority target |
| LinuxCNC | Open-source CNC controller | EMC2 compatible |
| Fanuc | Industrial mills | Industry standard |
| Haas | Haas mills and lathes | Popular in shops |
| Mach3/Mach4 | Windows-based CNC | Hobbyist/prosumer |

### Post-Processor Architecture

```rust
pub trait PostProcessor {
    fn header(&self, job: &CamJob) -> String;
    fn tool_change(&self, tool: &ToolEntry) -> String;
    fn segment(&self, seg: &ToolpathSegment, state: &mut PostState) -> String;
    fn footer(&self) -> String;
}
```

Each post handles dialect differences:
- Coordinate format (decimal places, leading zeros)
- Arc format (IJK incremental vs absolute, R format)
- Canned cycles (G81-G89 variations)
- Tool change sequences
- Coolant/spindle codes

## Stock Simulation

### SDF Octree

Efficient representation for subtractive manufacturing:

```rust
impl StockOctree {
    /// Initialize from stock bounding box
    pub fn from_bounds(bounds: Aabb, resolution: f64) -> Self;

    /// Subtract swept tool volume
    pub fn subtract_toolpath(&mut self, path: &[ToolpathSegment], tool: &Tool);

    /// Extract visualization mesh
    pub fn to_mesh(&self) -> TriangleMesh;

    /// Check for remaining material in region
    pub fn has_material(&self, region: &Aabb) -> bool;
}
```

### GPU Acceleration

Leverage vcad's existing wgpu infrastructure:

- Compute shader for SDF updates (parallel voxel processing)
- Marching cubes on GPU for real-time visualization
- Target: 60fps interactive simulation

## WASM Considerations

All CAM code must compile to WASM for browser execution:

- **No FFI**: Cannot link C++ libraries directly. Port or reimplement in Rust.
- **No threads**: Use async chunking for long calculations.
- **Memory**: Watch allocation size; octrees can grow large.

### Async Chunking Pattern

```rust
pub async fn generate_toolpath(
    op: &CamOperation,
    progress: impl Fn(f32),
) -> Result<Toolpath> {
    let chunks = op.decompose_chunks();
    let mut result = Toolpath::new();

    for (i, chunk) in chunks.iter().enumerate() {
        result.extend(process_chunk(chunk)?);
        progress(i as f32 / chunks.len() as f32);
        yield_now().await;  // Allow UI updates
    }

    Ok(result)
}
```

## MCP Tools

Expose CAM functionality to AI agents:

### `create_cam_job`

```json
{
  "document": "document_id",
  "stock": { "type": "box", "x": 100, "y": 100, "z": 25 },
  "workholding": { "type": "vise", "jaw_height": 15 }
}
```

### `add_operation`

```json
{
  "job": "job_id",
  "type": "adaptive_clearing",
  "tool": { "type": "flat_endmill", "diameter": 6 },
  "params": { "optimal_load": 0.4, "stepdown": 3 }
}
```

### `generate_toolpath`

```json
{
  "job": "job_id",
  "operation": "op_id"
}
```

Returns toolpath statistics (length, time estimate, warnings).

### `export_gcode`

```json
{
  "job": "job_id",
  "post": "grbl",
  "output": "part.nc"
}
```

### `simulate_machining`

```json
{
  "job": "job_id",
  "step": 100
}
```

Returns stock state after N toolpath segments.

## Implementation Phases

### Phase 1: Foundation (2 months)

- [ ] `vcad-kernel-cam` crate scaffold
- [ ] Tool definitions and toolpath IR
- [ ] 2.5D operations: Face, Pocket2D, Contour2D
- [ ] GRBL post-processor
- [ ] G-code export (IR → file)
- [ ] Basic app UI: operation list, tool library

**Deliverable**: Users can generate simple 2.5D toolpaths and export G-code.

### Phase 2: 3D Roughing (2 months)

- [ ] Port drop-cutter from OpenCAMLib
- [ ] Adaptive clearing algorithm
- [ ] Stock simulation MVP (CPU octree)
- [ ] Collision detection (tool/holder vs stock)
- [ ] LinuxCNC post-processor

**Deliverable**: 3D roughing with basic stock visualization.

### Phase 3: 3D Finishing (2 months)

- [ ] Parallel (raster) finishing
- [ ] Waterline (constant-Z) finishing
- [ ] Scallop (3D offset) finishing
- [ ] Pencil tracing (corner cleanup)
- [ ] Surface quality estimation

**Deliverable**: Full 3D finishing strategies.

### Phase 4: Drilling & Features (2 months)

- [ ] Hole feature recognition from BRep
- [ ] Drilling cycles (peck, tap, bore)
- [ ] Automatic operation planning
- [ ] Setup sheets (HTML/PDF export)
- [ ] Fanuc and Haas post-processors

**Deliverable**: Automatic hole detection and drilling.

### Phase 5: Production (4 months)

- [ ] GPU-accelerated stock simulation
- [ ] Real-time toolpath preview
- [ ] 3+2 indexed machining
- [ ] Tool library persistence
- [ ] Machine configuration profiles
- [ ] G-code verification (backplot)
- [ ] Remaining post-processors

**Deliverable**: Production-ready CAM system.

## Performance Targets

| Operation | Input Size | Target Time |
|-----------|------------|-------------|
| 2D pocket | 100mm² region | <100ms |
| Adaptive clearing | 1000 triangles | <5s |
| 3D parallel finish | 10000 triangles | <10s |
| Stock simulation update | Single segment | <16ms (60fps) |
| G-code export | 100K lines | <500ms |

## Document Format Extension

CAM data stored in `.vcad` documents:

```json
{
  "cam": {
    "jobs": [{
      "id": "job-1",
      "part": "part-id",
      "stock": { "type": "box", "bounds": [...] },
      "operations": [{
        "id": "op-1",
        "type": "adaptive_clearing",
        "tool": 1,
        "params": { "optimal_load": 0.4, "stepdown": 3 }
      }],
      "tools": [{
        "number": 1,
        "type": "flat_endmill",
        "diameter": 6,
        "flute_length": 20
      }]
    }]
  }
}
```

## References

- OpenCAMLib: https://github.com/aewallin/opencamlib
- Clipper2: https://github.com/AngusJohnson/Clipper2
- LinuxCNC G-code reference: https://linuxcnc.org/docs/html/gcode/g-code.html
- GRBL G-code subset: https://github.com/gnea/grbl/wiki/Grbl-v1.1-Commands
