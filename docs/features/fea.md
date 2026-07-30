# FEA (Finite Element Analysis) + Lattice Optimization

> **Superseded by a shipped implementation that diverged from this design.** Structural FEA shipped as `crates/vcad-kernel-fea` (structured lattice tet fill + matrix-free CG solve + fail-closed convergence receipts — not the Bowyer-Watson/Delaunay + nalgebra-sparse design below), surfaced in the app's Analyze mode and the `analyze_structure` MCP tool. Density-based optimization shipped separately as `crates/vcad-kernel-topopt` (SIMP, `topology_optimize` MCP tool). The `vcad-kernel-lattice` TPMS crate and the `run_fea`/`optimize_lattice`/`generate_lattice` MCP tools described here were never built. See `docs/fea-m0.md`.

## Status

| Field | Value |
|-------|-------|
| State | `shipped` (core FEA; lattice infill not built) |
| Owner | `n/a` |
| Priority | `p1` |
| Effort | `n/a` |

> Verified against repo 2026-07-30. Added missing Status table; doc's design diverged from shipped code (see banner above).

## Overview

vcad implements a pure-Rust, browser-native FEA system with integrated lattice optimization. This is the first step toward "design at any scale" — enabling users to define loads and constraints, then automatically optimize internal structure for weight reduction.

## Architecture

All computation runs in the browser via WASM. No server required.

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   BRep      │────▶│   Delaunay  │────▶│   PCG       │────▶│  Visualize  │
│   Model     │     │   Tet Mesh  │     │   Solver    │     │  Three.js   │
└─────────────┘     └─────────────┘     └─────────────┘     └─────────────┘
                          │                   │
                          ▼                   ▼
                    Materials/BCs         Stress Field
                    Loads                      │
                                              ▼
                                    ┌─────────────────┐
                                    │ Lattice Optimize│
                                    │ (SIMP-inspired) │
                                    └─────────────────┘
                                              │
                                              ▼
                                    ┌─────────────────┐
                                    │ TPMS + Marching │
                                    │     Cubes       │
                                    └─────────────────┘
```

## Architectural Decisions

1. **FEA solver**: Custom, built from scratch using `nalgebra-sparse` — full control, good for training data generation
2. **Mesh generation**: Custom Delaunay (Bowyer-Watson) in pure Rust — no external deps
3. **Material library**: Ship with defaults (steel, aluminum, titanium, ABS, PLA, nylon)
4. **Lattice export**: Implicit surface sampling (TPMS → marching cubes) for clean meshes

---

## Phase 1: vcad-kernel-fea (Tet Mesh + Linear Elastic Solver)

### Crate Structure

```
crates/vcad-kernel-fea/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API
│   ├── mesh/
│   │   ├── mod.rs
│   │   ├── point.rs        # 3D point with ID
│   │   ├── tetrahedron.rs  # Tet element
│   │   ├── delaunay.rs     # Bowyer-Watson algorithm
│   │   └── boundary.rs     # Surface mesh → tet mesh
│   ├── element/
│   │   ├── mod.rs
│   │   ├── shape.rs        # Shape functions (linear tet)
│   │   ├── stiffness.rs    # Element stiffness matrix (12x12)
│   │   └── strain.rs       # Strain-displacement matrix B
│   ├── assembly/
│   │   ├── mod.rs
│   │   ├── global.rs       # Global stiffness assembly
│   │   ├── boundary.rs     # Apply BCs (Dirichlet, Neumann)
│   │   └── load.rs         # Load vector assembly
│   ├── solver/
│   │   ├── mod.rs
│   │   ├── sparse.rs       # CSR matrix wrapper
│   │   ├── conjugate.rs    # Conjugate gradient solver
│   │   └── precond.rs      # Incomplete Cholesky preconditioner
│   ├── postproc/
│   │   ├── mod.rs
│   │   ├── stress.rs       # Compute stress tensor from displacement
│   │   ├── vonmises.rs     # Von Mises equivalent stress
│   │   └── interpolate.rs  # Nodal → surface interpolation
│   └── result.rs           # FeaResult struct
```

### Core Data Structures

```rust
// mesh/tetrahedron.rs
pub struct TetMesh {
    pub nodes: Vec<Point3>,           // Node positions
    pub elements: Vec<[usize; 4]>,    // Tet connectivity (node indices)
    pub surface_map: Vec<usize>,      // Surface vertex → tet node mapping
}

// element/stiffness.rs
/// 12x12 element stiffness matrix for linear tetrahedron
/// K_e = V * B^T * D * B
/// where V = tet volume, B = strain-displacement, D = material matrix
pub fn element_stiffness(
    nodes: [Point3; 4],
    material: &Material,
) -> Matrix12x12;

// Material matrix D for isotropic linear elasticity
pub fn material_matrix(youngs: f64, poisson: f64) -> Matrix6x6;

// assembly/global.rs
pub struct GlobalSystem {
    pub stiffness: CsrMatrix<f64>,    // Global K (sparse)
    pub load: DVector<f64>,           // Global F
    pub fixed_dofs: HashSet<usize>,   // Constrained DOFs
}

// result.rs
pub struct FeaResult {
    pub displacement: Vec<Vec3>,      // Per-node displacement
    pub stress: Vec<StressTensor>,    // Per-element stress
    pub vonmises: Vec<f64>,           // Per-element von Mises
    pub max_stress: f64,
    pub max_displacement: f64,
    pub strain_energy: f64,
}
```

### Delaunay Tetrahedralization (Bowyer-Watson)

```rust
// mesh/delaunay.rs
pub fn tetrahedralize(boundary_mesh: &TriMesh) -> TetMesh {
    // 1. Compute bounding super-tetrahedron
    // 2. Insert boundary vertices one by one
    //    - Find tets whose circumsphere contains new point
    //    - Remove bad tets, form cavity
    //    - Retriangulate cavity with new point
    // 3. Refine interior with Steiner points for quality
    // 4. Remove super-tet vertices
    // 5. Ensure boundary conformity
}

/// Quality metric: ratio of inscribed to circumscribed sphere radius
pub fn tet_quality(nodes: [Point3; 4]) -> f64;

/// Refine until all tets meet quality threshold
pub fn refine_mesh(mesh: &mut TetMesh, min_quality: f64, max_volume: f64);
```

### Solver (Preconditioned Conjugate Gradient)

```rust
// solver/conjugate.rs
pub struct PcgSolver {
    pub max_iterations: usize,    // default: 10000
    pub tolerance: f64,           // default: 1e-10
    pub preconditioner: Preconditioner,
}

impl PcgSolver {
    /// Solve K * u = F for displacement u
    pub fn solve(
        &self,
        stiffness: &CsrMatrix<f64>,
        load: &DVector<f64>,
        fixed_dofs: &HashSet<usize>,
    ) -> Result<DVector<f64>, SolverError>;
}

// solver/precond.rs
pub enum Preconditioner {
    None,
    Jacobi,                       // Diagonal scaling
    IncompleteCholesky { fill: usize },
}
```

### Public API

```rust
// lib.rs
pub fn run_analysis(
    solid: &Solid,
    material: &Material,
    boundary_conditions: &[BoundaryCondition],
    params: &MeshParams,
) -> Result<FeaResult, FeaError>;

pub struct MeshParams {
    pub target_element_size: f64,     // mm
    pub min_quality: f64,             // 0.0-1.0
    pub max_elements: usize,          // safety limit
}
```

### Tests

```rust
#[cfg(test)]
mod tests {
    // Cantilever beam: analytical solution δ = FL³/3EI
    #[test]
    fn cantilever_beam_deflection();

    // Uniaxial tension: σ = F/A
    #[test]
    fn uniaxial_tension_stress();

    // Pressurized sphere: analytical hoop stress
    #[test]
    fn pressurized_sphere();
}
```

---

## Phase 2: IR Types

### Rust IR (`crates/vcad-ir/src/lib.rs`)

```rust
// ─── Materials ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Material {
    pub id: String,
    pub name: String,
    pub density: f64,           // kg/m³
    pub youngs_modulus: f64,    // Pa (e.g., 200e9 for steel)
    pub poissons_ratio: f64,    // 0.0-0.5 (e.g., 0.3 for steel)
    pub yield_strength: f64,    // Pa
    pub color: Option<[f32; 3]>,
}

/// Built-in material library
pub fn default_materials() -> Vec<Material> {
    vec![
        Material {
            id: "steel-1018".into(),
            name: "Steel 1018".into(),
            density: 7870.0,
            youngs_modulus: 205e9,
            poissons_ratio: 0.29,
            yield_strength: 370e6,
            color: Some([0.7, 0.7, 0.75]),
        },
        Material {
            id: "aluminum-6061".into(),
            name: "Aluminum 6061-T6".into(),
            density: 2700.0,
            youngs_modulus: 68.9e9,
            poissons_ratio: 0.33,
            yield_strength: 276e6,
            color: Some([0.85, 0.85, 0.88]),
        },
        Material {
            id: "titanium-6al4v".into(),
            name: "Titanium 6Al-4V".into(),
            density: 4430.0,
            youngs_modulus: 113.8e9,
            poissons_ratio: 0.342,
            yield_strength: 880e6,
            color: Some([0.75, 0.75, 0.78]),
        },
        Material {
            id: "abs".into(),
            name: "ABS Plastic".into(),
            density: 1040.0,
            youngs_modulus: 2.3e9,
            poissons_ratio: 0.35,
            yield_strength: 40e6,
            color: Some([0.2, 0.2, 0.2]),
        },
        Material {
            id: "pla".into(),
            name: "PLA".into(),
            density: 1250.0,
            youngs_modulus: 3.5e9,
            poissons_ratio: 0.36,
            yield_strength: 60e6,
            color: Some([0.95, 0.95, 0.9]),
        },
        Material {
            id: "nylon-pa12".into(),
            name: "Nylon PA12".into(),
            density: 1010.0,
            youngs_modulus: 1.7e9,
            poissons_ratio: 0.4,
            yield_strength: 48e6,
            color: Some([0.9, 0.9, 0.85]),
        },
    ]
}

// ─── Boundary Conditions ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LoadType {
    /// Concentrated force on face centroid or distributed
    #[serde(rename_all = "camelCase")]
    Force {
        direction: [f64; 3],  // unit vector
        magnitude: f64,       // Newtons
        distributed: bool,    // true = per-area, false = total
    },
    /// Uniform pressure normal to face
    #[serde(rename_all = "camelCase")]
    Pressure {
        magnitude: f64,       // Pa (positive = into surface)
    },
    /// Fixed in all DOFs (welded)
    Fixed,
    /// Fixed in translation, free rotation (pinned)
    Pinned,
    /// Symmetry plane (normal displacement = 0)
    Symmetric,
    /// Prescribed displacement
    #[serde(rename_all = "camelCase")]
    Displacement {
        value: [f64; 3],      // mm
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BoundaryCondition {
    pub id: String,
    pub name: String,
    pub face_ids: Vec<String>,  // Face IDs from BRep
    pub load_type: LoadType,
}

// ─── Lattice Configuration ───────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LatticeType {
    /// Simple cubic struts
    Cubic,
    /// Octet truss (face-centered + body-centered)
    Octet,
    /// Gyroid TPMS (smooth, isotropic)
    Gyroid,
    /// Schwarz P TPMS
    SchwarzP,
    /// Diamond TPMS
    Diamond,
    /// Random Voronoi cells
    Voronoi,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum DensityField {
    /// Uniform density everywhere
    #[serde(rename_all = "camelCase")]
    Uniform { density: f64 },  // 0.0-1.0

    /// Density varies with distance from surface
    #[serde(rename_all = "camelCase")]
    GradientFromSurface {
        surface_density: f64,
        core_density: f64,
    },

    /// Density from FEA stress field
    #[serde(rename_all = "camelCase")]
    StressBased {
        min_density: f64,
        max_density: f64,
        stress_field_id: String,  // Reference to FeaResult
    },

    /// Explicit voxel grid
    #[serde(rename_all = "camelCase")]
    Voxel {
        resolution: [usize; 3],
        values: Vec<f64>,         // Flattened 3D array
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LatticeConfig {
    pub lattice_type: LatticeType,
    pub cell_size: f64,           // mm
    pub density_field: DensityField,
    pub shell_thickness: f64,     // mm, outer skin
    pub min_wall_thickness: f64,  // mm, minimum strut/wall
}

// ─── Analysis Case ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MeshParams {
    pub target_element_size: f64,   // mm
    pub min_quality: f64,           // 0.0-1.0, default 0.3
    pub max_elements: usize,        // default 100_000
    pub refinement_regions: Vec<RefinementRegion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefinementRegion {
    pub face_ids: Vec<String>,
    pub element_size: f64,          // mm
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnalysisCase {
    pub id: String,
    pub name: String,
    pub part_id: String,
    pub material_id: String,
    pub boundary_conditions: Vec<BoundaryCondition>,
    pub mesh_params: MeshParams,
    pub lattice: Option<LatticeConfig>,
}

// ─── Results ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FeaResult {
    pub id: String,
    pub analysis_case_id: String,
    pub timestamp: String,

    // Summary metrics
    pub max_vonmises_stress: f64,     // Pa
    pub max_displacement: f64,         // mm
    pub total_strain_energy: f64,      // J
    pub safety_factor: f64,            // yield_strength / max_stress
    pub mass: f64,                     // kg

    // Per-node data (for visualization)
    pub node_positions: Vec<[f64; 3]>,
    pub node_displacements: Vec<[f64; 3]>,
    pub node_vonmises: Vec<f64>,       // Interpolated from elements

    // Mesh info
    pub num_nodes: usize,
    pub num_elements: usize,
    pub solve_time_ms: u64,
}
```

### TypeScript IR (`packages/ir/src/index.ts`)

Mirror all types with TypeScript interfaces using same camelCase naming.

---

## Phase 3: WASM Bindings

### New Functions (`crates/vcad-kernel-wasm/src/lib.rs`)

```rust
/// Run FEA on a solid body
#[wasm_bindgen]
pub async fn run_fea(
    kernel: &Kernel,
    solid_id: &str,
    material: JsValue,              // Material
    boundary_conditions: JsValue,   // Vec<BoundaryCondition>
    mesh_params: JsValue,           // MeshParams
    progress_callback: Option<js_sys::Function>,
) -> Result<JsValue, JsValue>;      // FeaResult

/// Generate lattice infill for a solid
#[wasm_bindgen]
pub async fn generate_lattice(
    kernel: &Kernel,
    solid_id: &str,
    config: JsValue,                // LatticeConfig
    progress_callback: Option<js_sys::Function>,
) -> Result<String, JsValue>;       // New solid ID

/// Compute optimal density field from FEA results
#[wasm_bindgen]
pub fn compute_density_field(
    fea_result: JsValue,            // FeaResult
    target_weight_fraction: f64,    // 0.0-1.0
    min_safety_factor: f64,
) -> Result<JsValue, JsValue>;      // DensityField

/// Get stress values for visualization (per-vertex colors)
#[wasm_bindgen]
pub fn get_stress_colors(
    fea_result: JsValue,            // FeaResult
    color_scale: &str,              // "rainbow" | "thermal" | "diverging"
    min_value: Option<f64>,
    max_value: Option<f64>,
) -> Result<Vec<f32>, JsValue>;     // RGBA per vertex

/// Get deformed mesh for visualization
#[wasm_bindgen]
pub fn get_deformed_mesh(
    kernel: &Kernel,
    solid_id: &str,
    fea_result: JsValue,            // FeaResult
    scale: f64,                     // Deformation exaggeration
) -> Result<JsValue, JsValue>;      // Mesh data
```

### Progress Callback Protocol

```typescript
interface ProgressUpdate {
  stage: 'meshing' | 'assembly' | 'solving' | 'postprocess';
  progress: number;  // 0.0-1.0
  message: string;
}
```

---

## Phase 4: Frontend - Analysis Panel

### Store (`packages/app/src/stores/analysis-store.ts`)

```typescript
import { create } from 'zustand';
import { subscribeWithSelector } from 'zustand/middleware';

interface LoadCase {
  id: string;
  name: string;
  faceIds: string[];
  loadType: LoadType;
}

interface AnalysisState {
  // Panel state
  panelOpen: boolean;
  activeTab: 'loads' | 'materials' | 'lattice' | 'mesh';

  // Configuration
  materials: Material[];
  partMaterials: Record<string, string>;  // partId → materialId
  loadCases: LoadCase[];
  latticeConfig: LatticeConfig | null;
  meshParams: MeshParams;

  // Analysis state
  isRunning: boolean;
  progress: { stage: string; percent: number; message: string } | null;
  result: FeaResult | null;
  error: string | null;

  // Visualization
  visualizationMode: 'solid' | 'stress' | 'displacement' | 'density' | 'safety';
  colorScale: 'rainbow' | 'thermal' | 'diverging';
  deformationScale: number;
  showMesh: boolean;
}

interface AnalysisActions {
  // Panel
  openPanel(): void;
  closePanel(): void;
  setActiveTab(tab: AnalysisState['activeTab']): void;

  // Materials
  setPartMaterial(partId: string, materialId: string): void;
  addCustomMaterial(material: Material): void;

  // Load cases
  addLoadCase(loadCase: Omit<LoadCase, 'id'>): void;
  updateLoadCase(id: string, updates: Partial<LoadCase>): void;
  removeLoadCase(id: string): void;

  // Lattice
  setLatticeConfig(config: LatticeConfig | null): void;

  // Mesh
  setMeshParams(params: Partial<MeshParams>): void;

  // Analysis
  runAnalysis(): Promise<void>;
  cancelAnalysis(): void;
  optimizeLattice(targetWeightFraction: number, minSafetyFactor: number): Promise<void>;

  // Visualization
  setVisualizationMode(mode: AnalysisState['visualizationMode']): void;
  setColorScale(scale: AnalysisState['colorScale']): void;
  setDeformationScale(scale: number): void;
  setShowMesh(show: boolean): void;
}
```

### Components

```
packages/app/src/components/analysis/
├── AnalysisPanel.tsx           # Main panel with tabs
├── LoadCasesTab.tsx            # Define forces, constraints
├── LoadCaseCard.tsx            # Individual load case editor
├── MaterialsTab.tsx            # Material library + assignment
├── MaterialSelector.tsx        # Dropdown with material preview
├── LatticeTab.tsx              # Lattice type, density, shell
├── LatticePreview.tsx          # Wireframe preview of unit cell
├── MeshSettingsTab.tsx         # Element size, refinement
├── AnalysisResultsCard.tsx     # Summary metrics + actions
├── VisualizationControls.tsx   # Mode selector, color scale
└── index.ts                    # Exports
```

### User Flow

```
1. Click "+ Add Load" button
2. Select load type from dropdown (Force, Pressure, Fixed, etc.)
3. Click faces in viewport to assign (multi-select with Shift)
4. For Force: set direction (X/Y/Z buttons or custom), magnitude (ScrubInput)
5. For Pressure: set magnitude (ScrubInput)
6. Load case card shows face count, type icon, magnitude
7. Click trash icon to delete, or drag to reorder
```

---

## Phase 5: Visualization

### Stress Heatmap Shader

```glsl
// stress.vert
attribute float a_stress;
varying float v_stress;
varying vec3 v_normal;

void main() {
  v_stress = a_stress;
  v_normal = normalMatrix * normal;
  gl_Position = projectionMatrix * modelViewMatrix * vec4(position, 1.0);
}

// stress.frag
uniform float u_minStress;
uniform float u_maxStress;
uniform int u_colorScale;  // 0=rainbow, 1=thermal, 2=diverging

varying float v_stress;
varying vec3 v_normal;

vec3 rainbow(float t) {
  return vec3(
    clamp(1.5 - abs(4.0 * t - 3.0), 0.0, 1.0),
    clamp(1.5 - abs(4.0 * t - 2.0), 0.0, 1.0),
    clamp(1.5 - abs(4.0 * t - 1.0), 0.0, 1.0)
  );
}

vec3 thermal(float t) {
  // Blue → White → Red
  return mix(
    mix(vec3(0.0, 0.0, 1.0), vec3(1.0, 1.0, 1.0), t * 2.0),
    mix(vec3(1.0, 1.0, 1.0), vec3(1.0, 0.0, 0.0), t * 2.0 - 1.0),
    step(0.5, t)
  );
}

void main() {
  float t = clamp((v_stress - u_minStress) / (u_maxStress - u_minStress), 0.0, 1.0);

  vec3 color;
  if (u_colorScale == 0) color = rainbow(t);
  else if (u_colorScale == 1) color = thermal(t);
  else color = thermal(t);

  // Simple lighting
  vec3 lightDir = normalize(vec3(1.0, 1.0, 1.0));
  float diff = max(dot(normalize(v_normal), lightDir), 0.3);

  gl_FragColor = vec4(color * diff, 1.0);
}
```

---

## Phase 6: vcad-kernel-lattice

### Crate Structure

```
crates/vcad-kernel-lattice/
├── Cargo.toml
├── src/
│   ├── lib.rs              # Public API
│   ├── tpms/
│   │   ├── mod.rs
│   │   ├── gyroid.rs       # sin(x)cos(y) + sin(y)cos(z) + sin(z)cos(x) = t
│   │   ├── schwarzp.rs     # cos(x) + cos(y) + cos(z) = t
│   │   ├── diamond.rs      # sin(x)sin(y)sin(z) + ... = t
│   │   └── eval.rs         # Common implicit evaluation
│   ├── strut/
│   │   ├── mod.rs
│   │   ├── cubic.rs        # Simple cubic struts
│   │   ├── octet.rs        # Octet truss
│   │   └── voronoi.rs      # Random Voronoi
│   ├── density/
│   │   ├── mod.rs
│   │   ├── field.rs        # 3D scalar field interface
│   │   ├── uniform.rs      # Constant density
│   │   ├── gradient.rs     # Distance-based gradient
│   │   └── stress.rs       # FEA-derived density
│   ├── meshing/
│   │   ├── mod.rs
│   │   ├── marching.rs     # Marching cubes
│   │   └── adaptive.rs     # Adaptive resolution
│   └── boolean/
│       ├── mod.rs
│       └── shell.rs        # Intersect lattice with shell
```

### TPMS Implicit Functions

```rust
// tpms/gyroid.rs
/// Gyroid: isotropic, smooth, good mechanical properties
/// Level set: sin(x)cos(y) + sin(y)cos(z) + sin(z)cos(x) = t
/// t controls wall thickness (t=0 is minimal surface)
pub fn gyroid(p: Vec3, cell_size: f64, t: f64) -> f64 {
    let k = TAU / cell_size;
    let x = p.x * k;
    let y = p.y * k;
    let z = p.z * k;

    x.sin() * y.cos() + y.sin() * z.cos() + z.sin() * x.cos() - t
}

/// Convert density (0-1) to threshold t
/// density=0.5 → t=0 (50% infill)
pub fn density_to_threshold(density: f64) -> f64 {
    // Empirically calibrated for gyroid
    1.4 * (0.5 - density)
}
```

### Public API

```rust
// lib.rs
pub fn generate_lattice(
    solid: &Solid,
    config: &LatticeConfig,
    progress: impl Fn(f32),
) -> Result<TriMesh, LatticeError>;

pub fn preview_lattice_cell(
    lattice_type: LatticeType,
    cell_size: f64,
    density: f64,
) -> TriMesh;
```

---

## Phase 7: Optimization Loop

### Algorithm (SIMP-inspired)

```rust
pub struct OptimizationParams {
    pub target_weight_fraction: f64,  // e.g., 0.6 = 40% weight reduction
    pub min_safety_factor: f64,       // e.g., 1.5
    pub max_iterations: usize,        // e.g., 10
    pub convergence_tolerance: f64,   // e.g., 0.01
}

pub fn optimize_lattice_density(
    solid: &Solid,
    material: &Material,
    boundary_conditions: &[BoundaryCondition],
    params: &OptimizationParams,
    progress: impl Fn(OptimizationProgress),
) -> Result<DensityField, OptimizationError> {
    // 1. Initialize uniform density field at target_weight_fraction
    let mut density = UniformDensityField::new(params.target_weight_fraction);

    for iter in 0..params.max_iterations {
        // 2. Run FEA with current density
        let fea_result = run_fea_with_variable_stiffness(
            solid, material, boundary_conditions, &density
        )?;

        // 3. Check convergence
        if fea_result.safety_factor >= params.min_safety_factor {
            let change = compute_density_change(&density, &prev_density);
            if change < params.convergence_tolerance {
                break;
            }
        }

        // 4. Update density based on strain energy sensitivity
        // High strain energy → increase density
        // Low strain energy → decrease density
        density = update_density(
            &density,
            &fea_result,
            params.target_weight_fraction,
        );

        progress(OptimizationProgress {
            iteration: iter,
            safety_factor: fea_result.safety_factor,
            weight_fraction: density.total_mass_fraction(),
        });
    }

    Ok(density)
}
```

---

## Phase 8: Polish & Documentation

### Error Handling

```rust
#[derive(Debug, thiserror::Error)]
pub enum FeaError {
    #[error("Mesh generation failed: {0}")]
    MeshingFailed(String),

    #[error("Singular stiffness matrix - check constraints")]
    SingularMatrix,

    #[error("Solver did not converge after {0} iterations")]
    SolverNotConverged(usize),

    #[error("No boundary conditions defined")]
    NoBoundaryConditions,

    #[error("Insufficient constraints - model is not fully constrained")]
    UnderConstrained,

    #[error("Element quality too low: {0:.2} < {1:.2}")]
    PoorMeshQuality(f64, f64),
}
```

### Performance Targets

| Operation | Target | Measurement |
|-----------|--------|-------------|
| Mesh generation (10K elements) | < 2s | `console.time` |
| FEA solve (10K elements) | < 3s | `console.time` |
| Lattice generation (1M triangles) | < 5s | `console.time` |
| Stress visualization update | < 16ms | Must not drop frames |

---

## Verification Checklist

- [ ] `cargo test -p vcad-kernel-fea` - All solver tests pass
- [ ] `cargo test -p vcad-kernel-lattice` - Lattice generation tests pass
- [ ] `cargo clippy --workspace -- -D warnings` - No warnings
- [ ] Cantilever beam deflection within 5% of analytical
- [ ] UI: Can define load case by clicking faces
- [ ] UI: Can run analysis and see stress heatmap
- [ ] UI: Can optimize lattice and export STL
- [ ] Performance: 10K element analysis < 5s total
- [ ] Latticed STL opens correctly in slicer (PrusaSlicer/Cura)

---

## MCP Tools

### run_fea

Run structural analysis on a solid body.

```typescript
interface RunFeaParams {
  solidId: string;
  materialId: string;
  boundaryConditions: BoundaryCondition[];
  meshParams?: MeshParams;
}

interface RunFeaResult {
  maxVonmisesStress: number;  // Pa
  maxDisplacement: number;     // mm
  safetyFactor: number;
  mass: number;                // kg
  solveTimeMs: number;
}
```

### optimize_lattice

Optimize lattice density field based on FEA results.

```typescript
interface OptimizeLatticeParams {
  solidId: string;
  materialId: string;
  boundaryConditions: BoundaryCondition[];
  targetWeightFraction: number;  // 0.0-1.0
  minSafetyFactor: number;
}

interface OptimizeLatticeResult {
  densityField: DensityField;
  finalSafetyFactor: number;
  finalWeightFraction: number;
  iterations: number;
}
```

### generate_lattice

Generate latticed solid from density field.

```typescript
interface GenerateLatticeParams {
  solidId: string;
  latticeConfig: LatticeConfig;
}

interface GenerateLatticeResult {
  newSolidId: string;
  triangleCount: number;
  volume: number;  // mm³
}
```
