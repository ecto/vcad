# PCB/Electronics Design (ECAD)

> **This plan is superseded by a diverged shipped implementation.** ECAD shipped, but not as specified below: it spans 12 `vcad-ecad-*` crates (`pcb`, `schematic`, `symbols`, `export`, `fabprep`, `parts`, `sim`, `verify`, `diff`, `package`, `source`, `diff-wasm`) with a **custom Rust autorouter** — Freerouting/Specctra DSN was never integrated — plus DRC, ERC, copper pours, impedance/SI, DFM, and Gerber/KiCad export. The MCP surface is far larger than the table below (`create_schematic`, `place_components`, `route_nets`, `route_diff_pair`, `run_drc`, `run_erc`, `fix_drc`, `fab_prep`, `export_gerber`, `export_kicad`, `import_kicad`, `import_eagle`, `board_from_solid`/`solid_from_board`, …). Treat the code and root CLAUDE.md as authoritative; the design details below are historical.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | @cam |

> Verified against repo 2026-07-30.

## Overview

PCB design support in vcad targets hardware and IoT users who need a unified MCAD-ECAD workflow. Rather than switching between separate mechanical and electronics tools, designers can work on enclosures, mounting, and circuit boards in a single environment with native 3D integration.

## Strategy

- **Custom Rust core** for tight vcad integration and consistent architecture
- **Reuse KiCad libraries** (20k+ symbols, 6k+ footprints) under CC-BY-SA 4.0
- **Integrate Freerouting** for production-quality autorouting via Specctra DSN

## Architecture

```
Schematic Capture → Netlist → PCB Layout → DRC → Fabrication Output
                                              ↓
                              3D Integration (PCB as solid in assembly)
```

The pipeline follows industry-standard ECAD workflow: capture schematics with symbols, generate netlists, place footprints on the PCB, route traces, validate with design rules, and export fabrication files. The key differentiator is native 3D integration—the PCB becomes a solid body in mechanical assemblies.

## New Crates and Packages

| Crate | Purpose |
|-------|---------|
| `vcad-ecad-ir` | Schematic and PCB data model, serialization |
| `vcad-ecad-symbols` | Symbol and footprint storage, KiCad library parsing |
| `vcad-ecad-pcb` | Copper pour algorithms, DRC engine |
| `vcad-ecad-schematic` | Netlist generation, ERC validation |
| `vcad-ecad-export` | Gerber, Excellon drill, pick-and-place output |

TypeScript packages mirror the Rust crates for app integration:
- `@vcad/ecad-ir` — TypeScript types for schematic and PCB data
- `@vcad/ecad-app` — React components for schematic and PCB editors

## Data Model

### Schematic

```rust
pub struct SchematicComponent {
    pub reference: String,           // "U1", "R1", "C1"
    pub symbol_lib_id: LibraryId,    // Reference to symbol library
    pub footprint_lib_id: LibraryId, // Reference to footprint library
    pub position: Vec2,
    pub rotation: f64,
    pub mirror: bool,
    pub value: String,               // "10k", "100nF", "ATmega328P"
    pub properties: HashMap<String, String>,
}

pub struct SchematicWire {
    pub start: Vec2,
    pub end: Vec2,
}

pub struct SchematicJunction {
    pub position: Vec2,
}

pub struct SchematicLabel {
    pub position: Vec2,
    pub text: String,
    pub label_type: LabelType, // Local, Global, Hierarchical
}

pub enum PinType {
    Input,
    Output,
    Bidirectional,
    TriState,
    Passive,
    PowerInput,
    PowerOutput,
    OpenCollector,
    OpenEmitter,
    Unconnected,
    Free,
}
```

### PCB

```rust
pub enum PcbLayer {
    // Copper layers
    FCu,                    // Front copper
    BCu,                    // Back copper
    In1Cu, In2Cu, /* ... */ In30Cu, // Inner copper layers

    // Silkscreen
    FSilkS, BSilkS,

    // Solder mask
    FMask, BMask,

    // Paste
    FPaste, BPaste,

    // Fabrication
    FAdhes, BAdhes,
    FCrtYd, BCrtYd,
    FFab, BFab,

    // Mechanical
    EdgeCuts,
    Margin,
    Dwgs_User,
    Cmts_User,
    Eco1_User,
    Eco2_User,
}

pub struct Pad {
    pub number: String,
    pub pad_type: PadType,      // THT, SMD, NPTH
    pub shape: PadShape,        // Circle, Rect, Oval, RoundRect, Custom
    pub position: Vec2,
    pub size: Vec2,
    pub drill: Option<DrillSpec>,
    pub net: Option<NetId>,
    pub layers: Vec<PcbLayer>,
}

pub struct Trace {
    pub start: Vec2,
    pub end: Vec2,
    pub width: f64,
    pub layer: PcbLayer,
    pub net: NetId,
}

pub struct TraceArc {
    pub center: Vec2,
    pub radius: f64,
    pub start_angle: f64,
    pub end_angle: f64,
    pub width: f64,
    pub layer: PcbLayer,
    pub net: NetId,
}

pub struct Via {
    pub position: Vec2,
    pub diameter: f64,
    pub drill: f64,
    pub layers: (PcbLayer, PcbLayer), // Start and end layers
    pub net: NetId,
}

pub struct Zone {
    pub outline: Vec<Vec2>,
    pub net: Option<NetId>,
    pub layer: PcbLayer,
    pub clearance: f64,
    pub min_thickness: f64,
    pub thermal_gap: f64,
    pub thermal_bridge_width: f64,
    pub fill_type: ZoneFillType, // Solid, Hatched
    pub priority: u32,
}

pub struct Footprint {
    pub reference: String,
    pub value: String,
    pub position: Vec2,
    pub rotation: f64,
    pub layer: PcbLayer,        // FCu or BCu
    pub pads: Vec<Pad>,
    pub graphics: Vec<FootprintGraphic>,
    pub model_3d: Option<PathBuf>, // STEP model path
    pub model_offset: Vec3,
    pub model_rotation: Vec3,
    pub model_scale: Vec3,
}
```

## Core Features

### Design Rule Check (DRC)

DRC validates the PCB against manufacturing constraints:

| Rule | Description |
|------|-------------|
| Min trace width | Minimum copper trace width |
| Clearance | Minimum distance between copper features |
| Via diameter | Minimum via outer diameter |
| Via drill | Minimum via drill diameter |
| Annular ring | Minimum copper ring around drilled holes |
| Edge clearance | Minimum distance from board edge |
| Hole-to-hole | Minimum distance between drilled holes |
| Silkscreen clearance | Minimum distance from silkscreen to pads |

Net class rules allow different constraints per net (e.g., wider traces for power).

```rust
pub struct DesignRules {
    pub default: NetClassRules,
    pub net_classes: HashMap<String, NetClassRules>,
}

pub struct NetClassRules {
    pub min_trace_width: f64,
    pub min_clearance: f64,
    pub min_via_diameter: f64,
    pub min_via_drill: f64,
    pub min_annular_ring: f64,
}
```

### Electrical Rules Check (ERC)

ERC validates schematic connectivity:

- **Unconnected pins** — Pins that should be connected but are not
- **Pin type compatibility** — Output driving output, input floating, etc.
- **Power net integrity** — Power pins properly connected to power nets
- **Missing power symbols** — Components without power connections
- **Duplicate references** — Multiple components with same reference designator

### Copper Pour

Zone fill algorithm:

1. Start with zone outline polygon
2. Subtract clearance buffers around all other-net copper features
3. Subtract keepout zones
4. For same-net pads, create thermal relief patterns (cross-shaped gaps)
5. Remove isolated islands below minimum area threshold
6. Optionally hatch instead of solid fill

```rust
pub fn fill_zone(zone: &Zone, board: &Pcb) -> Vec<Polygon> {
    let mut fill = zone.outline.clone();

    // Subtract clearances from other nets
    for feature in board.copper_features() {
        if feature.net != zone.net {
            fill = fill.difference(&feature.buffer(zone.clearance));
        }
    }

    // Create thermal reliefs for same-net pads
    for pad in board.pads_on_layer(zone.layer) {
        if pad.net == zone.net {
            fill = fill.difference(&thermal_relief(pad, zone));
        }
    }

    // Remove small islands
    fill.retain(|poly| poly.area() >= MIN_ISLAND_AREA);

    fill
}
```

## Autorouting

vcad integrates Freerouting for autorouting via the Specctra DSN format:

1. **Export DSN** — Convert PCB to Specctra Design format
2. **Run Freerouting** — Execute autorouter (Java, can run as subprocess or WASM)
3. **Import SES** — Parse Specctra Session file with routed traces

```rust
pub fn autoroute(pcb: &mut Pcb, options: &AutorouteOptions) -> Result<()> {
    let dsn = export_dsn(pcb)?;
    let ses = freerouting::route(&dsn, options)?;
    import_ses(pcb, &ses)?;
    Ok(())
}
```

Manual routing features:

- **Push-and-shove** — Move existing traces to make room for new ones
- **Interactive length tuning** — Meander patterns for matched-length signals
- **Differential pair routing** — Route paired traces with constant spacing

## MCAD-ECAD Integration

### IDX Format

IDX (Incremental Data Exchange) enables bidirectional sync between MCAD and ECAD:

```xml
<IDX version="1.0">
  <BoardOutline>
    <Polygon>...</Polygon>
  </BoardOutline>
  <MountingHoles>
    <Hole x="5.0" y="5.0" diameter="3.2"/>
  </MountingHoles>
  <KeepoutZones>
    <Zone type="component" height="10.0">...</Zone>
  </KeepoutZones>
  <ComponentPlacements>
    <Component refdes="U1" x="25.0" y="30.0" rotation="0" side="top"/>
  </ComponentPlacements>
</IDX>
```

Synchronized data:
- Board outline and cutouts
- Mounting hole positions and sizes
- Component keepout zones (height restrictions)
- Component positions and orientations
- Connector locations

### PCB as 3D Solid

Convert PCB to BRep solid for mechanical assembly:

```rust
pub fn pcb_to_solid(pcb: &Pcb) -> Solid {
    // Extrude board outline to thickness
    let board = extrude(&pcb.outline, pcb.thickness);

    // Subtract mounting holes and cutouts
    let board = pcb.holes.iter().fold(board, |b, hole| {
        b.subtract(&cylinder(hole.position, hole.diameter / 2.0, pcb.thickness))
    });

    // Place component 3D models
    let mut assembly = Assembly::new();
    assembly.add_part("board", board);

    for footprint in &pcb.footprints {
        if let Some(model_path) = &footprint.model_3d {
            let model = import_step(model_path)?;
            let placed = model
                .scale(footprint.model_scale)
                .rotate(footprint.model_rotation)
                .translate(footprint.model_offset)
                .rotate_z(footprint.rotation)
                .translate(footprint.position.extend(pcb.thickness));
            assembly.add_part(&footprint.reference, placed);
        }
    }

    assembly
}
```

## File Formats

### Output Formats

| Format | Extension | Purpose |
|--------|-----------|---------|
| Gerber RS-274X | `.gbr` | Fabrication layers (copper, mask, silkscreen) |
| Excellon | `.drl` | Drill files |
| Pick-and-place | `.csv` | Assembly machine coordinates |
| BOM | `.csv` | Bill of materials |
| IPC-2581 | `.xml` | Unified fabrication data |

### Gerber Layer Mapping

| Layer | Gerber File |
|-------|-------------|
| FCu | `*-F_Cu.gbr` |
| BCu | `*-B_Cu.gbr` |
| In1Cu | `*-In1_Cu.gbr` |
| FSilkS | `*-F_SilkS.gbr` |
| BSilkS | `*-B_SilkS.gbr` |
| FMask | `*-F_Mask.gbr` |
| BMask | `*-B_Mask.gbr` |
| EdgeCuts | `*-Edge_Cuts.gbr` |

### Pick-and-Place Format

```csv
Ref,Val,Package,PosX,PosY,Rot,Side
U1,ATmega328P,TQFP-32,25.4,30.0,0,top
R1,10k,0402,10.0,15.0,90,top
C1,100nF,0402,12.0,15.0,90,top
```

## Component Libraries

### KiCad Libraries

Primary source for symbols and footprints:

- **Repository:** `gitlab.com/kicad/libraries`
- **License:** CC-BY-SA 4.0
- **Symbols:** 20,000+ components
- **Footprints:** 6,000+ packages
- **3D Models:** STEP and VRML

Library structure:
```
kicad-symbols/
├── Amplifier_Audio.kicad_sym
├── Connector_Generic.kicad_sym
├── MCU_Microchip_ATmega.kicad_sym
└── ...

kicad-footprints/
├── Capacitor_SMD.pretty/
├── Package_QFP.pretty/
├── Connector_PinHeader_2.54mm.pretty/
└── ...
```

### Additional Sources

- **SnapEDA** — Free component models, various licenses
- **Ultra Librarian** — Manufacturer-provided models
- **SamacSys** — Large component database
- **Component Search Engine** — Aggregated search

## Implementation Phases

### Phase 1: Foundation (4-6 weeks)

- Define IR types for schematic and PCB data model
- Implement KiCad symbol and footprint parsers
- Basic schematic editor with component placement
- Wire drawing and junction handling
- Netlist generation

### Phase 2: PCB Core (6-8 weeks)

- PCB editor with layer management
- Footprint placement from netlist
- Manual trace routing
- Via placement
- Basic DRC (clearance, trace width)
- Ratsnest display

### Phase 3: Advanced PCB (4-6 weeks)

- Copper pour with thermal relief
- Freerouting integration
- Push-and-shove routing
- Differential pair routing
- Length tuning
- Full DRC suite

### Phase 4: Manufacturing (3-4 weeks)

- Gerber RS-274X export
- Excellon drill file export
- Pick-and-place CSV export
- BOM generation
- Fabrication preview (Gerber viewer)

### Phase 5: 3D Integration (4-6 weeks)

- PCB to BRep solid conversion
- Component STEP model placement
- IDX import/export
- Assembly integration
- Collision detection for enclosure design

### Phase 6: Polish (Ongoing)

- Hierarchical schematics
- Design variants and configurations
- SPICE simulation integration
- Signal integrity analysis
- Multi-board projects
- Version control integration

## MCP Tools

ECAD operations exposed via MCP for AI-assisted design:

| Tool | Purpose |
|------|---------|
| `create_schematic` | Create schematic from component list and connections |
| `place_components` | Auto-place footprints on PCB |
| `route_nets` | Route specific nets or run autorouter |
| `run_drc` | Execute design rule check |
| `run_erc` | Execute electrical rules check |
| `export_gerber` | Generate fabrication files |
| `pcb_to_3d` | Convert PCB to 3D solid for assembly |

## References

- KiCad file formats: https://dev-docs.kicad.org/en/file-formats/
- Gerber RS-274X: https://www.ucamco.com/gerber
- Freerouting: https://github.com/freerouting/freerouting
- IDF/IDX: https://www.simplifiedsolutionsinc.com/idf.htm
