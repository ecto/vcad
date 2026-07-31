# URDF Import/Export

Standard robotics format support for interoperability with ROS, Gazebo, and other robotics toolchains.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` |
| Owner | `unassigned` |
| Priority | `p1` |
| Effort | `s` (1 week) |
| Target | Robotics Phase 1 |

> Verified against repo 2026-07-30. Correction: State was `proposed` but this is shipped — `crates/vcad-kernel-urdf` exists (`reader.rs`, `writer.rs`, `types.rs`, `error.rs`), the CLI has `import-urdf` (`ImportUrdf` in `crates/vcad-cli/src/main.rs`) and URDF export (`export_urdf`), and root CLAUDE.md lists `vcad-kernel-urdf` (URDF robot description import) as a workspace crate. Task checkboxes below ticked where verifiably implemented; sections describing the crate as future work are historical design notes.

## Problem

Robotics engineers use URDF (Unified Robot Description Format) as the standard format for describing robot models. It's the native format for:

- **ROS/ROS2**: The Robot Operating System, used in most research and industrial robots
- **Gazebo**: Industry-standard physics simulator
- **MoveIt**: Motion planning framework
- **PyBullet, Isaac Sim, Drake**: Other major simulators

Currently, vcad assemblies cannot interoperate with the robotics ecosystem:

1. Robotics teams cannot import existing robot models (e.g., Panda, UR5, Kuka) into vcad
2. Assemblies designed in vcad cannot be tested in simulation
3. Teams are forced to maintain parallel model definitions in vcad and URDF
4. This friction prevents adoption of vcad in robotics workflows

Without URDF support, vcad is invisible to the large robotics community that needs CAD tools.

## Solution

New crate `vcad-kernel-urdf` for reading and writing URDF XML files, with full round-trip support for standard robot descriptions.

### Type Mappings

**Joints: URDF to vcad**

| URDF Joint | vcad JointKind | Notes |
|------------|----------------|-------|
| `fixed` | `Fixed` | No relative motion |
| `revolute` | `Revolute` | With `limits` mapped to vcad limits |
| `continuous` | `Revolute` | No limits (continuous rotation) |
| `prismatic` | `Slider` | With `limits` mapped to vcad limits |
| `floating` | — | Not supported (6 DOF, rare) |
| `planar` | — | Not supported (2 DOF translation) |

**Joints: vcad to URDF**

| vcad JointKind | URDF Joint | Notes |
|----------------|------------|-------|
| `Fixed` | `fixed` | Direct mapping |
| `Revolute` | `revolute` or `continuous` | `continuous` if no limits |
| `Slider` | `prismatic` | Direct mapping |
| `Cylindrical` | — | Export as `revolute` (loses translation DOF) |
| `Ball` | — | Export as `fixed` with warning |

**Geometry: URDF to vcad**

| URDF Geometry | vcad Primitive |
|---------------|----------------|
| `<box size="x y z"/>` | `Cuboid { x, y, z }` |
| `<cylinder radius="r" length="l"/>` | `Cylinder { r, height: l }` |
| `<sphere radius="r"/>` | `Sphere { r }` |
| `<mesh filename="..."/>` | External mesh reference (future) |

**Geometry: vcad to URDF**

| vcad Primitive | URDF Geometry |
|----------------|---------------|
| `Cuboid` | `<box>` |
| `Cylinder` | `<cylinder>` |
| `Sphere` | `<sphere>` |
| `Cone` | Tessellate to mesh, export with `<mesh>` |
| Complex/Boolean | Tessellate to mesh, export with `<mesh>` |

### CLI Commands

```bash
# Import URDF to vcad document
vcad import-urdf robot.urdf output.vcad

# Export vcad assembly to URDF
vcad export input.vcad output.urdf --format urdf

# Import with optional mesh directory
vcad import-urdf robot.urdf output.vcad --mesh-dir ./meshes
```

### Not Included in MVP

- `<gazebo>` plugin elements (Gazebo-specific extensions)
- `<transmission>` elements (motor/actuator definitions)
- `<sensor>` elements (cameras, IMUs, etc.)
- Material textures (only solid colors)
- Mesh file conversion (meshes referenced by path only)

## UX Details

### Import Flow (CLI)

1. Parse URDF XML file
2. Resolve mesh file paths relative to URDF location
3. Create vcad `PartDef` for each `<link>` with visual geometry
4. Create vcad `Instance` for each `<link>`
5. Create vcad `Joint` for each `<joint>` with mapped type
6. Set base link as `groundInstanceId`
7. Write output `.vcad` file

### Export Flow (CLI)

1. Load vcad document
2. Validate assembly structure (tree topology required)
3. Generate `<link>` for each instance
4. Generate `<joint>` for each vcad joint
5. For complex geometry, tessellate and write STL mesh files
6. Write URDF XML with mesh references

### Error Handling

| Condition | Behavior |
|-----------|----------|
| Missing mesh file | Warning, skip visual geometry |
| Unsupported joint type | Warning, export as `fixed` |
| Cyclic joint chain | Error, abort export |
| Invalid URDF XML | Parse error with line number |
| Missing `<robot>` element | Error, not a valid URDF file |

### Edge Cases

- **Multiple robots in directory**: Each `.urdf` file is a separate import
- **Relative mesh paths**: Resolved relative to URDF file location
- **Package URIs** (`package://...`): Resolve via `--ros-pkg-path` flag or skip with warning
- **Inertial properties**: Preserved as metadata (not used in vcad yet)

## Implementation

### Crate Structure

```
crates/vcad-kernel-urdf/
├── Cargo.toml
├── src/
│   ├── lib.rs           # Public API: read_urdf(), write_urdf()
│   ├── types.rs         # URDF data structures
│   ├── reader.rs        # XML parsing, conversion to vcad
│   ├── writer.rs        # vcad to XML serialization
│   └── tests.rs         # Unit tests with sample URDF files
```

### Dependencies

```toml
[dependencies]
quick-xml = "0.31"       # XML parsing
vcad-ir = { path = "../vcad-ir" }
vcad-kernel = { path = "../vcad-kernel" }
thiserror = "1.0"
```

### Types

```rust
// types.rs

/// URDF robot model
pub struct UrdfRobot {
    pub name: String,
    pub links: Vec<UrdfLink>,
    pub joints: Vec<UrdfJoint>,
    pub materials: Vec<UrdfMaterial>,
}

/// URDF link (part)
pub struct UrdfLink {
    pub name: String,
    pub visual: Option<UrdfVisual>,
    pub collision: Option<UrdfCollision>,
    pub inertial: Option<UrdfInertial>,
}

/// URDF joint
pub struct UrdfJoint {
    pub name: String,
    pub joint_type: UrdfJointType,
    pub parent: String,
    pub child: String,
    pub origin: Transform3D,
    pub axis: Vec3,
    pub limits: Option<UrdfJointLimits>,
}

pub enum UrdfJointType {
    Fixed,
    Revolute,
    Continuous,
    Prismatic,
    Floating,
    Planar,
}

pub struct UrdfJointLimits {
    pub lower: f64,
    pub upper: f64,
    pub effort: f64,
    pub velocity: f64,
}

pub enum UrdfGeometry {
    Box { size: Vec3 },
    Cylinder { radius: f64, length: f64 },
    Sphere { radius: f64 },
    Mesh { filename: String, scale: Option<Vec3> },
}
```

### Public API

```rust
// lib.rs

/// Read URDF file and convert to vcad Document
pub fn read_urdf(path: &Path) -> Result<Document, UrdfError>;

/// Read URDF from string
pub fn read_urdf_from_str(urdf_xml: &str) -> Result<Document, UrdfError>;

/// Write vcad Document to URDF file
pub fn write_urdf(doc: &Document, path: &Path) -> Result<(), UrdfError>;

/// Write vcad Document to URDF string
pub fn write_urdf_to_string(doc: &Document) -> Result<String, UrdfError>;
```

### CLI Integration

Add to `crates/vcad-cli/src/main.rs`:

```rust
#[derive(Subcommand)]
enum Commands {
    // ... existing commands ...

    /// Import URDF robot description
    ImportUrdf {
        /// Input URDF file
        input: PathBuf,
        /// Output vcad file
        output: PathBuf,
        /// Directory containing mesh files
        #[arg(long)]
        mesh_dir: Option<PathBuf>,
    },
}
```

### Files to Modify

| File | Changes |
|------|---------|
| `crates/vcad-kernel-urdf/` | New crate (entire implementation) |
| `crates/vcad-kernel/Cargo.toml` | Add `vcad-kernel-urdf` dependency |
| `crates/vcad-kernel/src/lib.rs` | Re-export URDF functions |
| `crates/vcad-cli/Cargo.toml` | Add `vcad-kernel-urdf` dependency |
| `crates/vcad-cli/src/main.rs` | Add `import-urdf` command, add URDF to `export` |
| `Cargo.toml` (workspace) | Add `vcad-kernel-urdf` to members |

## Tasks

### Phase 1: Core Types and Parsing

- [x] Create `vcad-kernel-urdf` crate with Cargo.toml (`xs`)
- [x] Define URDF types in `types.rs` (`s`)
- [x] Implement XML parser in `reader.rs` (`s`)
- [x] Add `read_urdf()` and `read_urdf_from_str()` functions (`xs`)

### Phase 2: Import Conversion

- [x] Map URDF joints to vcad JointKind (`s`)
- [x] Map URDF geometry to vcad primitives (`s`)
- [x] Build vcad Document from UrdfRobot (`s`)
- [ ] Handle mesh file references (store as metadata) (`xs`)

### Phase 3: Export

- [x] Implement `writer.rs` with XML serialization (`s`)
- [x] Map vcad joints back to URDF joint types (`xs`)
- [ ] Generate mesh files for complex geometry (`m`)
- [x] Add `write_urdf()` and `write_urdf_to_string()` functions (`xs`)

### Phase 4: CLI Integration

- [x] Add `import-urdf` command to CLI (`xs`)
- [x] Add `--format urdf` option to `export` command (`xs`)
- [ ] Add `--mesh-dir` and `--ros-pkg-path` flags (`xs`)

### Phase 5: Testing

- [x] Add unit tests with minimal URDF samples (`s`)
- [ ] Test round-trip with Franka Panda URDF (`s`)
- [ ] Test round-trip with UR5 URDF (`s`)
- [ ] Add integration tests to CI (`xs`)

## Acceptance Criteria

- [ ] `vcad import-urdf panda.urdf output.vcad` produces valid vcad document
- [ ] `vcad import-urdf ur5.urdf output.vcad` produces valid vcad document
- [ ] Imported robot has correct joint types (revolute, prismatic, fixed)
- [ ] Joint limits are preserved on import
- [ ] Joint axes are correctly oriented
- [ ] `vcad export assembly.vcad robot.urdf --format urdf` produces valid URDF XML
- [ ] Exported URDF loads in ROS `robot_state_publisher` without errors
- [ ] Round-trip (URDF -> vcad -> URDF) preserves joint structure
- [ ] Standard robot URDFs (Panda, UR5, Kuka iiwa) pass round-trip test
- [ ] CLI shows helpful error messages for invalid URDF files

## Future Enhancements

This feature enables the broader robotics roadmap:

- [ ] **Phase 2: Physics Simulation** — Integrate physics engine (phyz) for dynamics
- [ ] **Phase 3: Gym Environment** — Export to Gymnasium/IsaacGym for RL training
- [ ] **Phase 4: Motion Planning** — Integrate with MoveIt for path planning

Additional URDF enhancements:

- [ ] Web UI for URDF import (drag-and-drop)
- [ ] Support `<gazebo>` plugin elements
- [ ] Support `<transmission>` elements for actuator modeling
- [ ] Support `<sensor>` elements (cameras, IMUs, lidar)
- [ ] XACRO macro expansion (URDF preprocessing)
- [ ] SDF format support (Gazebo native format)
- [ ] MJCF format support (MuJoCo)
- [ ] Automatic inertia calculation from geometry
- [ ] Material/texture import from URDF
